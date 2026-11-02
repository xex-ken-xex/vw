//! フル解像度の上限（`viewer.full_image_limit_mb`）に当たったとき、パソコンの空きメモリに
//! 余裕があれば、上限を上げるよう勧める。
//!
//! 起動のたびに調べて勧めないのは、上限に当たらない使い方の人にはうるさいだけだから。

/// 勧めるときの上限の刻み（MB）。
const STEP_MB: u64 = 512;

/// 空きメモリ（バイト）。調べられなければ `None`。
pub fn available_memory() -> Option<u64> {
    imp::available()
}

/// 空きメモリ `available`（バイト）のとき、上限 `limit_mb` を何 MB に上げるよう勧めるか。
/// 空きの半分を刻みで切り捨てた量で、今の上限より大きくならなければ勧めない。
pub fn suggestion(limit_mb: u32, available: u64, max_mb: u32) -> Option<u32> {
    let half_mb = available / 2 / (1024 * 1024);
    let suggested = (half_mb / STEP_MB * STEP_MB).min(u64::from(max_mb));
    (suggested > u64::from(limit_mb)).then_some(suggested as u32)
}

#[cfg(target_os = "linux")]
mod imp {
    pub fn available() -> Option<u64> {
        super::mem_available(&std::fs::read_to_string("/proc/meminfo").ok()?)
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    pub fn available() -> Option<u64> {
        // SAFETY: MEMORYSTATUSEX は数値だけの C の構造体で、全部 0 でも正しい値になる。
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: dwLength を入れた MEMORYSTATUSEX を渡す。
        (unsafe { GlobalMemoryStatusEx(&mut status) } != 0).then_some(status.ullAvailPhys)
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
mod imp {
    pub fn available() -> Option<u64> {
        None
    }
}

/// `/proc/meminfo` の `MemAvailable`（kB）をバイトで。`MemFree` を使わないのは、ファイルの
/// キャッシュなど、すぐ空けられる分が入らず、余裕を少なく見積もるため。
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn mem_available(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn suggests_half_of_the_free_memory_in_512_mb_steps() {
        assert_eq!(suggestion(256, 5 * GB, 1_048_576), Some(2560));
        assert_eq!(suggestion(256, 3 * GB, 1_048_576), Some(1536));
    }

    #[test]
    fn does_not_suggest_when_the_limit_is_already_that_high() {
        assert_eq!(suggestion(2560, 5 * GB, 1_048_576), None);
        assert_eq!(suggestion(512, GB, 1_048_576), None);
    }

    #[test]
    fn suggestion_stays_within_the_setting_range() {
        assert_eq!(suggestion(256, 64 * GB, 4096), Some(4096));
    }

    #[test]
    fn reads_mem_available_from_meminfo() {
        let text = "MemTotal:       16000000 kB\nMemFree:  100 kB\nMemAvailable:    8000000 kB\n";
        assert_eq!(mem_available(text), Some(8_000_000 * 1024));
        assert_eq!(mem_available("MemTotal: 1 kB\n"), None);
    }

    #[cfg(any(target_os = "linux", windows))]
    #[test]
    fn this_computer_reports_some_free_memory() {
        assert!(available_memory().is_some_and(|b| b > 0));
    }
}
