mod app;
mod control;
mod fan;
mod gpu;
mod processes;
mod smc;
mod temps;
mod ui;

use std::process::ExitCode;

fn model_name() -> String {
    let chip = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    let model = std::process::Command::new("sysctl")
        .args(["-n", "hw.model"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mac".into());
    match chip {
        Some(chip) => format!("{model} · {chip}"),
        None => model,
    }
}

fn print_help() {
    println!("macfan — control Mac fan speed from the terminal");
    println!();
    println!("usage: macfan [--list | --auto | --help]");
    println!();
    println!("  (no args)   launch the TUI (sudo required to change speeds)");
    println!("  --list      print fans and temperatures, then exit");
    println!("  --auto      restore all fans to automatic control, then exit");
    println!();
    println!("TUI keys: ↑↓ select fan/thermals, ←→ ±100 RPM, shift←→ ±500,");
    println!("          m manual/auto when a fan is selected,");
    println!("          c sort processes by CPU, g sort processes by GPU,");
    println!("          a all auto, f full blast, space toggle linked fans,");
    println!("          q quit (restores auto), Q quit keeping settings");
}

fn run(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }
    if let Some(arg) = args
        .iter()
        .find(|a| !matches!(a.as_str(), "--list" | "--auto"))
    {
        return Err(format!("unknown argument '{arg}' (try --help)").into());
    }

    let smc = smc::Smc::open()?;
    let fans = fan::discover(&smc)?;
    if fans.is_empty() {
        return Err("no fans found (is this a fanless Mac?)".into());
    }

    if args.iter().any(|a| a == "--auto") {
        if !smc::is_root() {
            return Err(
                "restoring automatic fan control requires root — run: sudo macfan --auto".into(),
            );
        }
        let problems = control::force_auto(&smc, &fans);
        if problems.is_empty() {
            println!("all fans restored to automatic control");
            return Ok(());
        }
        return Err(problems.join("; ").into());
    }

    if args.iter().any(|a| a == "--list") {
        println!("discovering sensors…");
        let temps = temps::Temps::discover(&smc);
        println!("{}", model_name());
        println!();
        for fan in &fans {
            let mode = match fan.mode {
                fan::FanMode::Manual => "MANUAL",
                fan::FanMode::System => "SYSTEM",
                fan::FanMode::Auto => "AUTO",
            };
            println!(
                "  {:<12} {:>5.0} RPM  target {:>5.0}  range {:.0}–{:.0}  [{mode}]",
                fan.name, fan.actual, fan.target, fan.min, fan.max,
            );
        }
        println!();
        match &temps.hottest {
            Some((key, t)) => println!(
                "  temps: avg {:.1}°C, hottest {t:.1}°C ({key}), {} sensors",
                temps.avg,
                temps.sensor_count(),
            ),
            None => println!("  temps: no readable sensors"),
        }
        return Ok(());
    }

    let temps = temps::Temps::discover(&smc);
    let controller = control::Controller::spawn(fans.clone())?;
    let mut terminal = ratatui::init();
    let mut app = app::App::new(smc, fans, temps, model_name(), controller);
    let result = app.run(&mut terminal);
    ratatui::restore();
    if app.keep_on_exit() {
        println!("keeping manual fan settings — run `sudo macfan --auto` to restore");
    } else {
        println!("restoring fans to automatic control…");
    }
    use std::io::Write;
    let _ = std::io::stdout().flush();
    drop(app);
    result
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("macfan: {e}");
            ExitCode::FAILURE
        }
    }
}
