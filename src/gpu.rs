use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::fmt;
use std::ptr;

type KernReturn = i32;
type MachPort = u32;
type IoObjectRaw = u32;
type CfIndex = isize;
type CfTypeId = usize;
type CfTypeRef = *const c_void;
type CfStringRef = *const c_void;
type CfArrayRef = *const c_void;
type CfDictionaryRef = *const c_void;
type CfNumberRef = *const c_void;

const CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;
const CF_NUMBER_SINT64_TYPE: isize = 4;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const u8) -> *mut c_void;
    fn IOServiceGetMatchingServices(
        main_port: MachPort,
        matching: *mut c_void,
        existing: *mut IoObjectRaw,
    ) -> KernReturn;
    fn IOIteratorNext(iterator: IoObjectRaw) -> IoObjectRaw;
    fn IOObjectRelease(object: IoObjectRaw) -> KernReturn;
    fn IOObjectConformsTo(object: IoObjectRaw, class_name: *const c_char) -> u32;
    fn IORegistryEntryGetChildIterator(
        entry: IoObjectRaw,
        plane: *const c_char,
        iterator: *mut IoObjectRaw,
    ) -> KernReturn;
    fn IORegistryEntryCreateCFProperty(
        entry: IoObjectRaw,
        key: CfStringRef,
        allocator: *const c_void,
        options: u32,
    ) -> CfTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(value: CfTypeRef);
    fn CFGetTypeID(value: CfTypeRef) -> CfTypeId;
    fn CFStringGetTypeID() -> CfTypeId;
    fn CFArrayGetTypeID() -> CfTypeId;
    fn CFDictionaryGetTypeID() -> CfTypeId;
    fn CFNumberGetTypeID() -> CfTypeId;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        value: *const c_char,
        encoding: u32,
    ) -> CfStringRef;
    fn CFStringGetCString(
        value: CfStringRef,
        buffer: *mut c_char,
        buffer_size: CfIndex,
        encoding: u32,
    ) -> u8;
    fn CFArrayGetCount(array: CfArrayRef) -> CfIndex;
    fn CFArrayGetValueAtIndex(array: CfArrayRef, index: CfIndex) -> CfTypeRef;
    fn CFDictionaryGetValue(dictionary: CfDictionaryRef, key: CfTypeRef) -> CfTypeRef;
    fn CFNumberGetValue(number: CfNumberRef, number_type: isize, value: *mut c_void) -> u8;
}

#[derive(Debug)]
pub enum GpuError {
    CoreFoundation,
    Registry(KernReturn),
    NoAccelerator,
    NoProcessCounters,
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::CoreFoundation => write!(f, "could not create CoreFoundation keys"),
            GpuError::Registry(code) => write!(f, "IORegistry query failed ({code:#x})"),
            GpuError::NoAccelerator => write!(f, "no IOAccelerator service found"),
            GpuError::NoProcessCounters => {
                write!(f, "the GPU driver exposes no per-process AppUsage counters")
            }
        }
    }
}

impl std::error::Error for GpuError {}

struct CfOwned(CfTypeRef);

impl CfOwned {
    fn string(value: &CStr) -> Result<CfOwned, GpuError> {
        let raw = unsafe {
            CFStringCreateWithCString(ptr::null(), value.as_ptr(), CF_STRING_ENCODING_UTF8)
        };
        if raw.is_null() {
            Err(GpuError::CoreFoundation)
        } else {
            Ok(CfOwned(raw))
        }
    }
}

impl Drop for CfOwned {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) };
    }
}

struct IoObject(IoObjectRaw);

impl Drop for IoObject {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { IOObjectRelease(self.0) };
        }
    }
}

pub struct GpuReader {
    creator_key: CfOwned,
    app_usage_key: CfOwned,
    accumulated_time_key: CfOwned,
}

impl GpuReader {
    pub fn new() -> Result<GpuReader, GpuError> {
        Ok(GpuReader {
            creator_key: CfOwned::string(c"IOUserClientCreator")?,
            app_usage_key: CfOwned::string(c"AppUsage")?,
            accumulated_time_key: CfOwned::string(c"accumulatedGPUTime")?,
        })
    }

    pub fn read_times(&self) -> Result<HashMap<u32, u64>, GpuError> {
        let matching = unsafe { IOServiceMatching(c"IOAccelerator".as_ptr().cast()) };
        if matching.is_null() {
            return Err(GpuError::NoAccelerator);
        }
        let mut accelerators = 0;
        let result = unsafe { IOServiceGetMatchingServices(0, matching, &mut accelerators) };
        if result != 0 {
            return Err(GpuError::Registry(result));
        }
        let accelerators = IoObject(accelerators);
        let mut found_accelerator = false;
        let mut found_counters = false;
        let mut totals = HashMap::new();

        while let Some(accelerator) = next_object(&accelerators) {
            found_accelerator = true;
            let mut children = 0;
            let result = unsafe {
                IORegistryEntryGetChildIterator(accelerator.0, c"IOService".as_ptr(), &mut children)
            };
            if result != 0 {
                continue;
            }
            let children = IoObject(children);
            while let Some(child) = next_object(&children) {
                let conforms = unsafe { IOObjectConformsTo(child.0, c"IOUserClient".as_ptr()) };
                if conforms == 0 {
                    continue;
                }
                let Some(pid) = self.pid_for_client(child.0) else {
                    continue;
                };
                let Some(time) = self.time_for_client(child.0) else {
                    continue;
                };
                found_counters = true;
                totals
                    .entry(pid)
                    .and_modify(|total: &mut u64| *total = total.saturating_add(time))
                    .or_insert(time);
            }
        }

        if !found_accelerator {
            Err(GpuError::NoAccelerator)
        } else if !found_counters {
            Err(GpuError::NoProcessCounters)
        } else {
            Ok(totals)
        }
    }

    fn pid_for_client(&self, client: IoObjectRaw) -> Option<u32> {
        let value = property(client, self.creator_key.0)?;
        cf_string(value.0).and_then(|creator| parse_creator_pid(&creator))
    }

    fn time_for_client(&self, client: IoObjectRaw) -> Option<u64> {
        let value = property(client, self.app_usage_key.0)?;
        if !has_type(value.0, unsafe { CFArrayGetTypeID() }) {
            return None;
        }
        let count = unsafe { CFArrayGetCount(value.0) };
        if count < 0 {
            return None;
        }
        let mut total = 0u64;
        for index in 0..count {
            let dictionary = unsafe { CFArrayGetValueAtIndex(value.0, index) };
            if !has_type(dictionary, unsafe { CFDictionaryGetTypeID() }) {
                continue;
            }
            let number = unsafe { CFDictionaryGetValue(dictionary, self.accumulated_time_key.0) };
            if let Some(time) = cf_u64(number) {
                total = total.saturating_add(time);
            }
        }
        Some(total)
    }
}

fn next_object(iterator: &IoObject) -> Option<IoObject> {
    let object = unsafe { IOIteratorNext(iterator.0) };
    (object != 0).then_some(IoObject(object))
}

fn property(entry: IoObjectRaw, key: CfStringRef) -> Option<CfOwned> {
    let value = unsafe { IORegistryEntryCreateCFProperty(entry, key, ptr::null(), 0) };
    if value.is_null() {
        None
    } else {
        Some(CfOwned(value))
    }
}

fn has_type(value: CfTypeRef, expected: CfTypeId) -> bool {
    !value.is_null() && unsafe { CFGetTypeID(value) } == expected
}

fn cf_string(value: CfTypeRef) -> Option<String> {
    if !has_type(value, unsafe { CFStringGetTypeID() }) {
        return None;
    }
    let mut buffer = [0 as c_char; 256];
    let converted = unsafe {
        CFStringGetCString(
            value,
            buffer.as_mut_ptr(),
            buffer.len() as CfIndex,
            CF_STRING_ENCODING_UTF8,
        )
    };
    if converted == 0 {
        return None;
    }
    Some(
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
    )
}

fn cf_u64(value: CfTypeRef) -> Option<u64> {
    if !has_type(value, unsafe { CFNumberGetTypeID() }) {
        return None;
    }
    let mut number = 0i64;
    let converted = unsafe {
        CFNumberGetValue(
            value,
            CF_NUMBER_SINT64_TYPE,
            &mut number as *mut i64 as *mut c_void,
        )
    };
    (converted != 0 && number >= 0).then_some(number as u64)
}

fn parse_creator_pid(creator: &str) -> Option<u32> {
    let rest = creator.strip_prefix("pid ")?;
    let digits = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_gpu_client_creator_pid() {
        assert_eq!(parse_creator_pid("pid 386, WindowServer"), Some(386));
        assert_eq!(parse_creator_pid("pid 17"), Some(17));
        assert_eq!(parse_creator_pid("WindowServer"), None);
    }

    #[test]
    #[ignore = "requires a macOS IOAccelerator service"]
    fn reads_live_gpu_counters() {
        let reader = GpuReader::new().unwrap();
        let times = reader.read_times().unwrap();
        assert!(!times.is_empty());
        eprintln!("read GPU counters for {} processes", times.len());
    }
}
