//! How warm this Mac runs: CPU, GPU and battery from the SMC's sensors, and the thermal state
//! macOS itself reports (when it is about to slow the Mac down). Read without root, the way
//! Activity Monitor's peers do; a Mac that won't say gets `None`, never a made-up figure.

use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct Temps {
    /// °C, the mean of the CPU's die sensors.
    pub cpu: Option<f64>,
    pub gpu: Option<f64>,
    pub battery: Option<f64>,
    /// "nominal", "fair", "serious" or "critical": macOS's thermal pressure, as
    /// NSProcessInfo.thermalState names it. Serious is where Apple asks apps to cut work.
    pub thermal: Option<&'static str>,
}

/// One read of every sensor Pitlane shows. The SMC's key list is walked once per daemon and
/// kept; later reads ask only the keys that matter.
pub fn read() -> Temps {
    #[cfg(target_os = "macos")]
    {
        let mut t = mac::sensors();
        t.thermal = mac::thermal_state();
        t
    }
    #[cfg(not(target_os = "macos"))]
    {
        Temps::default()
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::Temps;
    use std::ffi::{CStr, c_char, c_void};
    use std::sync::{Mutex, OnceLock};

    type KernReturn = i32;
    type MachPort = u32;

    #[link(name = "IOKit", kind = "framework")]
    unsafe extern "C" {
        fn IOServiceMatching(name: *const c_char) -> *mut c_void;
        fn IOServiceGetMatchingServices(main: MachPort, matching: *mut c_void, iter: *mut MachPort) -> KernReturn;
        fn IOIteratorNext(iter: MachPort) -> MachPort;
        fn IORegistryEntryGetName(entry: MachPort, name: *mut c_char) -> KernReturn;
        fn IOServiceOpen(service: MachPort, task: MachPort, kind: u32, conn: *mut MachPort) -> KernReturn;
        fn IOConnectCallStructMethod(
            conn: MachPort,
            selector: u32,
            input: *const c_void,
            input_len: usize,
            output: *mut c_void,
            output_len: *mut usize,
        ) -> KernReturn;
        fn IOObjectRelease(obj: MachPort) -> KernReturn;
    }

    unsafe extern "C" {
        static mach_task_self_: MachPort;
    }

    // The AppleSMC user client's in/out record (SMCKeyData_t), 80 bytes.
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Vers {
        major: u8,
        minor: u8,
        build: u8,
        reserved: u8,
        release: u16,
    }
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct PLimit {
        version: u16,
        length: u16,
        cpu: u32,
        gpu: u32,
        mem: u32,
    }
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct KeyInfo {
        size: u32,
        kind: u32,
        attributes: u8,
    }
    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct KeyData {
        key: u32,
        vers: Vers,
        limit: PLimit,
        info: KeyInfo,
        result: u8,
        status: u8,
        data8: u8,
        data32: u32,
        bytes: [u8; 32],
    }
    const _: () = assert!(std::mem::size_of::<KeyData>() == 80);

    const READ_BYTES: u8 = 5;
    const READ_INDEX: u8 = 8;
    const READ_INFO: u8 = 9;
    const FLT: u32 = u32::from_be_bytes(*b"flt ");
    const SP78: u32 = u32::from_be_bytes(*b"sp78");

    struct Smc {
        conn: MachPort,
        cpu: Vec<(u32, KeyInfo)>,
        gpu: Vec<(u32, KeyInfo)>,
        battery: Vec<(u32, KeyInfo)>,
    }

    impl Smc {
        fn open() -> Option<Smc> {
            let mut iter: MachPort = 0;
            // SAFETY: IOServiceMatching returns a dictionary that IOServiceGetMatchingServices
            // consumes; `iter` is written by the call and released below.
            let found = unsafe { IOServiceGetMatchingServices(0, IOServiceMatching(c"AppleSMC".as_ptr()), &mut iter) };
            if found != 0 {
                return None;
            }
            let mut conn: MachPort = 0;
            loop {
                // SAFETY: walks the iterator we own; each entry is released after use.
                let dev = unsafe { IOIteratorNext(iter) };
                if dev == 0 {
                    break;
                }
                let mut name = [0 as c_char; 128];
                // SAFETY: io_name_t is 128 bytes; the call writes a NUL-terminated name into it.
                let named = unsafe { IORegistryEntryGetName(dev, name.as_mut_ptr()) } == 0;
                // SAFETY: `name` is NUL-terminated when the call above succeeded.
                let keys = named && unsafe { CStr::from_ptr(name.as_ptr()) }.to_bytes() == b"AppleSMCKeysEndpoint";
                // Intel Macs have a single AppleSMC with no endpoint child: take the first one.
                if conn == 0 || keys {
                    let mut c: MachPort = 0;
                    // SAFETY: opens a user client on a service we hold; `c` is written on success.
                    if unsafe { IOServiceOpen(dev, mach_task_self_, 0, &mut c) } == 0 {
                        conn = c;
                    }
                }
                // SAFETY: releases the entry IOIteratorNext handed us.
                unsafe { IOObjectRelease(dev) };
                if keys && conn != 0 {
                    break;
                }
            }
            // SAFETY: the iterator is ours.
            unsafe { IOObjectRelease(iter) };
            if conn == 0 {
                return None;
            }
            let mut smc = Smc { conn, cpu: vec![], gpu: vec![], battery: vec![] };
            let count = smc.bytes(u32::from_be_bytes(*b"#KEY")).map_or(0, |(b, _)| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
            for i in 0..count.min(4096) {
                let Some(key) = smc.call(KeyData { data8: READ_INDEX, data32: i, ..Default::default() }).map(|d| d.key) else {
                    continue;
                };
                let [a, b, ..] = key.to_be_bytes();
                let part = match (a, b) {
                    // Apple silicon: Tp/Te are the performance and efficiency cores; Intel: TC.
                    (b'T', b'p' | b'e' | b'C') => 0,
                    (b'T', b'g' | b'G') => 1,
                    (b'T', b'B') => 2,
                    _ => continue,
                };
                let Some(info) = smc.info(key).filter(|i| i.kind == FLT || i.kind == SP78) else {
                    continue;
                };
                match part {
                    0 => smc.cpu.push((key, info)),
                    1 => smc.gpu.push((key, info)),
                    _ => smc.battery.push((key, info)),
                }
            }
            Some(smc)
        }

        fn call(&self, input: KeyData) -> Option<KeyData> {
            let mut out = KeyData::default();
            let mut len = std::mem::size_of::<KeyData>();
            // SAFETY: both records are SMCKeyData_t-sized; selector 2 is the SMC's read-write
            // call, which fills `out` and never writes past `len`.
            let r = unsafe {
                IOConnectCallStructMethod(
                    self.conn,
                    2,
                    (&input as *const KeyData).cast(),
                    std::mem::size_of::<KeyData>(),
                    (&mut out as *mut KeyData).cast(),
                    &mut len,
                )
            };
            (r == 0 && out.result == 0).then_some(out)
        }

        fn info(&self, key: u32) -> Option<KeyInfo> {
            self.call(KeyData { key, data8: READ_INFO, ..Default::default() }).map(|d| d.info)
        }

        fn bytes(&self, key: u32) -> Option<([u8; 32], KeyInfo)> {
            let info = self.info(key)?;
            self.call(KeyData { key, info, data8: READ_BYTES, ..Default::default() }).map(|d| (d.bytes, info))
        }

        fn celsius(&self, key: u32, info: KeyInfo) -> Option<f64> {
            let d = self.call(KeyData { key, info, data8: READ_BYTES, ..Default::default() })?;
            let b = d.bytes;
            let v = match info.kind {
                FLT => f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])),
                _ => f64::from(i16::from_be_bytes([b[0], b[1]])) / 256.0,
            };
            // Unplugged or idle sensors read 0 or garbage; a real die is somewhere in between.
            (10.0..130.0).contains(&v).then_some(v)
        }

        fn mean(&self, keys: &[(u32, KeyInfo)]) -> Option<f64> {
            let v: Vec<f64> = keys.iter().filter_map(|&(k, i)| self.celsius(k, i)).collect();
            (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
        }

        fn max(&self, keys: &[(u32, KeyInfo)]) -> Option<f64> {
            keys.iter().filter_map(|&(k, i)| self.celsius(k, i)).reduce(f64::max)
        }
    }

    pub(super) fn sensors() -> Temps {
        static SMC: OnceLock<Mutex<Option<Smc>>> = OnceLock::new();
        let smc = SMC.get_or_init(|| Mutex::new(Smc::open()));
        let Ok(smc) = smc.lock() else {
            return Temps::default();
        };
        let Some(smc) = smc.as_ref() else {
            return Temps::default();
        };
        Temps {
            cpu: smc.mean(&smc.cpu),
            gpu: smc.mean(&smc.gpu),
            battery: smc.max(&smc.battery),
            thermal: None,
        }
    }

    unsafe extern "C" {
        fn notify_register_check(name: *const c_char, token: *mut i32) -> u32;
        fn notify_get_state(token: i32, state: *mut u64) -> u32;
    }

    /// macOS's thermal pressure (`com.apple.system.thermalpressurelevel`), readable without root:
    /// 0 nominal, 1 moderate, 2 heavy, 3 trapping, 4 sleeping. Named the way
    /// `NSProcessInfo.thermalState` names them (heavy is serious; trapping and on are critical).
    pub(super) fn thermal_state() -> Option<&'static str> {
        static TOKEN: OnceLock<Option<i32>> = OnceLock::new();
        let token = (*TOKEN.get_or_init(|| {
            let mut t = 0;
            // SAFETY: registers a check token for a public notification name; `t` is written on
            // success and stays valid for the life of the process.
            (unsafe { notify_register_check(c"com.apple.system.thermalpressurelevel".as_ptr(), &mut t) } == 0).then_some(t)
        }))?;
        let mut level = 0u64;
        // SAFETY: reads the state behind a token we registered; `level` is a plain u64.
        if unsafe { notify_get_state(token, &mut level) } != 0 {
            return None;
        }
        Some(match level {
            0 => "nominal",
            1 => "fair",
            2 => "serious",
            _ => "critical",
        })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_without_failing() {
        let t = super::read();
        for v in [t.cpu, t.gpu, t.battery].into_iter().flatten() {
            assert!((10.0..130.0).contains(&v), "{v}");
        }
        #[cfg(target_os = "macos")]
        assert!(t.thermal.is_some());
    }
}
