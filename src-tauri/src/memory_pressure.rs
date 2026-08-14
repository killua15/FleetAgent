//! Whether the machine can currently afford an expensive background job.
//!
//! Hangar's quota check starts a whole Claude Code — measured at ~456 MB and
//! 20% of a core on the machine this was written for. On a laptop already
//! running two agents, an IDE and a browser, that periodic spike is what pushes
//! an over-committed system into paging, and a main thread waiting on a page
//! fault is exactly what macOS reports as "not responding".
//!
//! So the poller asks here first. This is not a memory *manager* — it is one
//! number, read without spawning anything, used to decide whether now is a good
//! moment to add half a gigabyte.

/// How much of the swap file is in use, 0.0-1.0. `None` when the platform
/// can't be asked, which is every platform except macOS today.
///
/// Swap rather than free RAM on purpose: macOS keeps free memory near zero by
/// design (it fills it with cache), so "free RAM" says nothing about pressure.
/// Swap in use is the honest signal that the machine has already run out and
/// started paying for it in disk.
#[cfg(target_os = "macos")]
pub fn swap_used_ratio() -> Option<f64> {
    // `struct xsw_usage` from <sys/sysctl.h>. Declared here rather than pulled
    // from a crate: it is four integers and a flag, and it has been stable for
    // the entire life of the platform.
    #[repr(C)]
    #[derive(Default)]
    struct XswUsage {
        total: u64,
        avail: u64,
        used: u64,
        pagesize: u32,
        encrypted: u8,
    }

    let mut usage = XswUsage::default();
    let mut size = std::mem::size_of::<XswUsage>();
    let name = c"vm.swapusage";

    // SAFETY: `sysctlbyname` writes at most `size` bytes into `usage`, and
    // `size` is that struct's own size. The name is a NUL-terminated literal.
    let ok = unsafe {
        libc::sysctlbyname(name.as_ptr(), &mut usage as *mut _ as *mut libc::c_void, &mut size, std::ptr::null_mut(), 0) == 0
    };

    if !ok || usage.total == 0 {
        return None;
    }
    Some(usage.used as f64 / usage.total as f64)
}

#[cfg(not(target_os = "macos"))]
pub fn swap_used_ratio() -> Option<f64> {
    None
}

/// Past this much swap in use, the machine is paying for memory in disk seeks
/// and a background job that allocates hundreds of megabytes will be felt.
const HEAVY_SWAP: f64 = 0.60;

/// Whether to back off expensive work right now.
///
/// Deliberately a *multiplier* on how often the poller runs rather than an
/// on/off switch — see `usage_scraper::cache_ttl`. A machine that is
/// permanently swapping (16 GB with an IDE and several agents is) would
/// otherwise never get a quota reading again, and the out-of-quota banner would
/// quietly stop working. Backing off degrades; switching off breaks.
pub fn under_pressure() -> bool {
    swap_used_ratio().is_some_and(|ratio| ratio >= HEAVY_SWAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Prints what the kernel says, to be read against `sysctl vm.swapusage`.
    ///
    /// Ignored because the numbers belong to whatever machine runs it, and a
    /// test that asserts on this machine's swap is a test that fails on
    /// somebody else's. It exists because the reader above is `unsafe` and
    /// decodes a C struct by hand: the way that goes wrong is silently, with a
    /// plausible-looking number, and the only way to catch it is to compare
    /// against the tool that reads the same sysctl.
    #[test]
    #[ignore = "prints this machine's swap — compare with `sysctl vm.swapusage`"]
    fn swap_reading_matches_the_kernel() {
        match swap_used_ratio() {
            Some(ratio) => {
                assert!((0.0..=1.0).contains(&ratio), "a ratio outside 0-1 means the struct is being decoded wrong: {ratio}");
                println!("swap in use: {:.1}%  ·  under_pressure() = {}", ratio * 100.0, under_pressure());
            }
            None => println!("no swap configured, or the platform can't be asked"),
        }
    }
}
