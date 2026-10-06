//! Hardware warnings: a notice, never a block.
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

const MIN_CORES: usize = 4;
const MIN_RAM_BYTES: u64 = 7_500_000_000; // 8 GB machines report slightly less than 8 GiB.

pub fn warnings(physical_cores: usize, ram_bytes: Option<u64>, avx2: bool) -> Vec<String> {
    let mut out = Vec::new();
    if physical_cores < MIN_CORES {
        out.push(format!(
            "This PC has {physical_cores} CPU cores; 4 or more are recommended. Captions may lag."
        ));
    }
    if !avx2 {
        out.push("This CPU lacks AVX2, so translation will be slow.".into());
    }
    if ram_bytes.is_some_and(|bytes| bytes < MIN_RAM_BYTES) {
        out.push("This PC has less than 8 GB of memory; 8 GB or more is recommended.".into());
    }
    out
}

fn installed_ram() -> Option<u64> {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    unsafe { GlobalMemoryStatusEx(&mut status) }
        .ok()
        .map(|()| status.ullTotalPhys)
}

pub fn current_warnings() -> Vec<String> {
    warnings(
        num_cpus::get_physical(),
        installed_ram(),
        std::is_x86_feature_detected!("avx2"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adequate_hardware_has_no_warnings() {
        assert!(warnings(8, Some(16_000_000_000), true).is_empty());
        assert!(warnings(4, Some(8_000_000_000), true).is_empty());
    }

    #[test]
    fn each_shortfall_is_reported_separately() {
        assert_eq!(warnings(2, Some(16_000_000_000), true).len(), 1);
        assert_eq!(warnings(8, Some(4_000_000_000), true).len(), 1);
        assert_eq!(warnings(8, Some(16_000_000_000), false).len(), 1);
        assert_eq!(warnings(2, Some(4_000_000_000), false).len(), 3);
        assert!(warnings(8, None, true).is_empty());
    }
}
