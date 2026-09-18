#![allow(clippy::unreadable_literal)]
use anyhow::{Result, bail};

use crate::ksu_uapi;
use std::cell::Cell;
use std::fs;
use std::io;
use std::os::fd::RawFd;
use std::sync::OnceLock;

// sigsys handler
std::thread_local! {
    #[allow(clippy::missing_const_for_thread_local)]
    static SVC_IN_FLIGHT: Cell<bool> = const { Cell::new(false) };
    #[allow(clippy::missing_const_for_thread_local)]
    static SIGSYS_OCCURRED: Cell<bool> = const { Cell::new(false) };
}

const SYS_SECCOMP: libc::c_int = 1;

fn with_svc_call<F, R>(call: F) -> R
where
    F: FnOnce() -> R,
{
    SVC_IN_FLIGHT.with(|in_flight| in_flight.set(true));
    let result = call();
    SVC_IN_FLIGHT.with(|in_flight| in_flight.set(false));
    result
}

fn take_sigsys_occurred() -> bool {
    SIGSYS_OCCURRED.with(|occurred| occurred.replace(false))
}

extern "C" fn sigsys_handler(
    _sig: libc::c_int,
    info: *mut libc::siginfo_t,
    ctx: *mut libc::c_void,
) {
    unsafe {
        if info.is_null() || ctx.is_null() || (*info).si_code != SYS_SECCOMP {
            return;
        }
        if SVC_IN_FLIGHT.with(Cell::get) {
            SIGSYS_OCCURRED.with(|occurred| occurred.set(true));
        }

        let ucontext = ctx.cast::<libc::ucontext_t>();
        #[cfg(target_arch = "aarch64")]
        {
            (*ucontext).uc_mcontext.regs[0] = (-libc::EPERM) as u64;
        }
        #[cfg(target_arch = "x86_64")]
        {
            let rax = libc::REG_RAX as usize;
            (*ucontext).uc_mcontext.gregs[rax] = i64::from(-libc::EPERM);
        }
    }
}

pub fn setup_sigsys_handler() {
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_flags = libc::SA_SIGINFO;
        sa.sa_sigaction = sigsys_handler as *const () as usize;
        libc::sigemptyset(std::ptr::addr_of_mut!(sa.sa_mask));
        if libc::sigaction(libc::SIGSYS, std::ptr::addr_of!(sa), std::ptr::null_mut()) != 0 {
            let error = std::io::Error::last_os_error();
            log::warn!("Failed to set SIGSYS handler: {error}");
        }
    }
}

const DRIVER_FD_NAME: &str = "anon_inode:[ksu_driver]";
const SU_DRIVER_FD_NAME: &str = "anon_inode:[ksu_driver_su]";

// Global driver fd cache
static DRIVER_FD: OnceLock<RawFd> = OnceLock::new();
static INFO_CACHE: OnceLock<ksu_uapi::ksu_get_info_cmd> = OnceLock::new();

fn scan_driver_fd() -> io::Result<Option<RawFd>> {
    let fd_dir = fs::read_dir("/proc/self/fd")?;
    let mut driver_fd = None;

    for entry in fd_dir.flatten() {
        if let Ok(fd_num) = entry.file_name().to_string_lossy().parse::<i32>() {
            let link_path = format!("/proc/self/fd/{fd_num}");
            if let Ok(target) = fs::read_link(&link_path) {
                let target_str = target.to_string_lossy();
                if target_str == SU_DRIVER_FD_NAME {
                    return Ok(Some(fd_num));
                }
                if target_str == DRIVER_FD_NAME {
                    driver_fd = Some(fd_num);
                }
            }
        }
    }

    Ok(driver_fd)
}

pub fn claim_inherited_driver_fd() -> io::Result<()> {
    if DRIVER_FD.get().is_none()
        && let Some(fd) = scan_driver_fd()?
    {
        let _ = DRIVER_FD.set(fd);
    }
    Ok(())
}

// Get cached driver fd
fn init_driver_fd() -> Option<RawFd> {
    let fd = scan_driver_fd().ok().flatten();
    if fd.is_none() {
        let mut fd = -1;
        with_svc_call(|| unsafe {
            libc::syscall(
                libc::SYS_reboot,
                ksu_uapi::KSU_INSTALL_MAGIC1,
                ksu_uapi::KSU_INSTALL_MAGIC2,
                0,
                &mut fd,
            )
        });
        if take_sigsys_occurred() {
            eprintln!("KernelSU driver install syscall was blocked by seccomp");
            log::error!("KernelSU driver install syscall was blocked by seccomp");
        }
        if fd >= 0 { Some(fd) } else { None }
    } else {
        fd
    }
}

// ioctl wrapper using libc
fn ksuctl<T>(request: u32, arg: *mut T) -> Result<i32> {
    use std::io;

    let fd = *DRIVER_FD.get_or_init(|| init_driver_fd().unwrap_or(-1));
    if fd < 0 {
        bail!("could not retrieve kernelsu driver fd")
    }
    // Retry on EAGAIN (os error 11) and EINTR (os error 4) - up to 5 times
    let mut last_err: Option<io::Error> = None;
    for attempt in 0..5 {
        unsafe {
            let ret = libc::ioctl(fd as libc::c_int, request as i32, arg);
            if ret >= 0 {
                return Ok(ret);
            }
            let err = io::Error::last_os_error();
            let raw = err.raw_os_error().unwrap_or(0);
            if (raw == 11 || raw == 4) && attempt < 4 {
                last_err = Some(err);
                std::thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }
            bail!("ksuctl failed: {}", err)
        }
    }
    bail!("ksuctl failed after retries: {}", last_err.unwrap())
}

// API implementations
pub fn get_info() -> ksu_uapi::ksu_get_info_cmd {
    *INFO_CACHE.get_or_init(|| {
        let mut cmd = ksu_uapi::ksu_get_info_cmd {
            version: 0,
            flags: 0,
            features: 0,
            uapi_version: 0,
        };
        if ksuctl(ksu_uapi::KSU_IOCTL_GET_INFO, &raw mut cmd).is_err() {
            let _ = ksuctl(ksu_uapi::KSU_IOCTL_GET_INFO_LEGACY, &raw mut cmd);
        }
        cmd
    })
}

pub fn get_version() -> i32 {
    get_info().version as i32
}

pub fn is_late_load() -> bool {
    get_info().flags & ksu_uapi::KSU_GET_INFO_FLAG_LATE_LOAD != 0
}

pub fn is_lkm() -> bool {
    get_info().flags & ksu_uapi::KSU_GET_INFO_FLAG_LKM != 0
}

pub const fn uapi_version() -> u32 {
    ksu_uapi::KERNEL_SU_UAPI_VERSION
}

pub fn runtime_mode() -> &'static str {
    if is_late_load() {
        "late-load"
    } else if is_lkm() {
        "lkm"
    } else {
        "built-in"
    }
}

pub fn ensure_uapi_version_matched() -> anyhow::Result<()> {
    let kernel_uapi = get_info().uapi_version;
    let userspace_uapi = uapi_version();
    if kernel_uapi != userspace_uapi {
        bail!(
            "UAPI version mismatch: kernel={kernel_uapi}, ksud={userspace_uapi}. Please update KernelSU!"
        );
    }
    Ok(())
}

pub fn grant_root() -> Result<()> {
    ksuctl(ksu_uapi::KSU_IOCTL_GRANT_ROOT, std::ptr::null_mut::<u8>())?;
    Ok(())
}

fn report_event(event: u32) {
    let mut cmd = ksu_uapi::ksu_report_event_cmd { event };
    let _ = ksuctl(ksu_uapi::KSU_IOCTL_REPORT_EVENT, &raw mut cmd);
}

pub fn report_post_fs_data() {
    report_event(ksu_uapi::EVENT_POST_FS_DATA);
}

pub fn report_boot_complete() {
    report_event(ksu_uapi::EVENT_BOOT_COMPLETED);
}

pub fn report_module_mounted() {
    report_event(ksu_uapi::EVENT_MODULE_MOUNTED);
}

pub fn check_kernel_safemode() -> bool {
    let mut cmd = ksu_uapi::ksu_check_safemode_cmd { in_safe_mode: 0 };
    let _ = ksuctl(ksu_uapi::KSU_IOCTL_CHECK_SAFEMODE, &raw mut cmd);
    cmd.in_safe_mode != 0
}

pub fn set_sepolicy(payload: *const u8, payload_len: u64) -> Result<i32> {
    let mut ioctl_cmd = crate::ksu_uapi::ksu_set_sepolicy_cmd {
        data_len: payload_len,
        data: payload as u64,
    };

    ksuctl(ksu_uapi::KSU_IOCTL_SET_SEPOLICY, &raw mut ioctl_cmd)
}

/// Get feature value and support status from kernel
/// Returns (value, supported)
pub fn get_feature(feature_id: u32) -> Result<(u64, bool)> {
    let mut cmd = ksu_uapi::ksu_get_feature_cmd {
        feature_id,
        value: 0,
        supported: 0,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_GET_FEATURE, &raw mut cmd)?;
    Ok((cmd.value, cmd.supported != 0))
}

/// Set feature value in kernel
/// Note: kernel may set the value successfully but return EAGAIN (os error 11).
/// We verify the value after setting and treat it as success if the value matches.
pub fn set_feature(feature_id: u32, value: u64) -> Result<()> {
    let mut cmd = ksu_uapi::ksu_set_feature_cmd { feature_id, value };
    let result = ksuctl(ksu_uapi::KSU_IOCTL_SET_FEATURE, &raw mut cmd);
    match result {
        Ok(_) => {
            log::info!("set_feature: id={feature_id} value={value} success");
            Ok(())
        }
        Err(e) => {
            let err_str = e.to_string();
            // Kernel bug: may return EAGAIN even when value was set successfully
            if err_str.contains("Try again") || err_str.contains("os error 11") {
                // Verify the value was actually set
                match get_feature(feature_id) {
                    Ok((actual_value, supported)) => {
                        if actual_value == value {
                            log::info!(
                                "set_feature: id={feature_id} value={value} ioctl returned EAGAIN but value verified, treating as success (supported={supported})"
                            );
                            return Ok(());
                        }
                        log::warn!(
                            "set_feature: id={feature_id} value={value} ioctl returned EAGAIN, actual value={actual_value} (mismatch), returning error"
                        );
                    }
                    Err(verify_err) => {
                        log::warn!(
                            "set_feature: id={feature_id} value={value} ioctl returned EAGAIN, verify failed: {verify_err}, returning original error"
                        );
                    }
                }
            }
            log::error!("set_feature: id={feature_id} value={value} failed: {e}");
            Err(e)
        }
    }
}

pub fn get_wrapped_fd(fd: RawFd) -> Result<RawFd> {
    let mut cmd = ksu_uapi::ksu_get_wrapper_fd_cmd {
        fd: fd as u32,
        flags: 0,
    };
    let result = ksuctl(ksu_uapi::KSU_IOCTL_GET_WRAPPER_FD, &raw mut cmd)?;
    Ok(result)
}

pub fn get_sulog_fd() -> Result<RawFd> {
    let mut cmd = ksu_uapi::ksu_get_sulog_fd_cmd { flags: 0 };
    let result = ksuctl(ksu_uapi::KSU_IOCTL_GET_SULOG_FD, &raw mut cmd)?;
    Ok(result)
}

/// Get mark status for a process (pid=0 returns total marked count)
pub fn mark_get(pid: i32) -> Result<u32> {
    let mut cmd = ksu_uapi::ksu_manage_mark_cmd {
        operation: ksu_uapi::KSU_MARK_GET,
        pid,
        result: 0,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_MANAGE_MARK, &raw mut cmd)?;
    Ok(cmd.result)
}

/// Mark a process (pid=0 marks all processes)
pub fn mark_set(pid: i32) -> Result<()> {
    let mut cmd = ksu_uapi::ksu_manage_mark_cmd {
        operation: ksu_uapi::KSU_MARK_MARK,
        pid,
        result: 0,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_MANAGE_MARK, &raw mut cmd)?;
    Ok(())
}

/// Unmark a process (pid=0 unmarks all processes)
pub fn mark_unset(pid: i32) -> Result<()> {
    let mut cmd = ksu_uapi::ksu_manage_mark_cmd {
        operation: ksu_uapi::KSU_MARK_UNMARK,
        pid,
        result: 0,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_MANAGE_MARK, &raw mut cmd)?;
    Ok(())
}

/// Refresh mark for all running processes
pub fn mark_refresh() -> Result<()> {
    let mut cmd = ksu_uapi::ksu_manage_mark_cmd {
        operation: ksu_uapi::KSU_MARK_REFRESH,
        pid: 0,
        result: 0,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_MANAGE_MARK, &raw mut cmd)?;
    Ok(())
}

pub fn nuke_ext4_sysfs(mnt: &str) -> anyhow::Result<()> {
    let c_mnt = std::ffi::CString::new(mnt)?;
    let mut ioctl_cmd = ksu_uapi::ksu_nuke_ext4_sysfs_cmd {
        arg: c_mnt.as_ptr() as u64,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_NUKE_EXT4_SYSFS, &raw mut ioctl_cmd)?;
    Ok(())
}

/// Wipe all entries from umount list
pub fn umount_list_wipe() -> Result<()> {
    let mut cmd = ksu_uapi::ksu_add_try_umount_cmd {
        arg: 0,
        flags: 0,
        mode: ksu_uapi::KSU_UMOUNT_WIPE,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_ADD_TRY_UMOUNT, &raw mut cmd)?;
    Ok(())
}

/// Add mount point to umount list
pub fn umount_list_add(path: &str, flags: u32) -> anyhow::Result<()> {
    let c_path = std::ffi::CString::new(path)?;
    let mut cmd = ksu_uapi::ksu_add_try_umount_cmd {
        arg: c_path.as_ptr() as u64,
        flags,
        mode: ksu_uapi::KSU_UMOUNT_ADD,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_ADD_TRY_UMOUNT, &raw mut cmd)?;
    Ok(())
}

/// Delete mount point from umount list
pub fn umount_list_del(path: &str) -> anyhow::Result<()> {
    let c_path = std::ffi::CString::new(path)?;
    let mut cmd = ksu_uapi::ksu_add_try_umount_cmd {
        arg: c_path.as_ptr() as u64,
        flags: 0,
        mode: ksu_uapi::KSU_UMOUNT_DEL,
    };
    ksuctl(ksu_uapi::KSU_IOCTL_ADD_TRY_UMOUNT, &raw mut cmd)?;
    Ok(())
}

/// Set current process's process group to init_group (pgid = 0)
pub fn set_init_pgrp() -> Result<()> {
    ksuctl(
        ksu_uapi::KSU_IOCTL_SET_INIT_PGRP,
        std::ptr::null_mut::<u8>(),
    )?;
    Ok(())
}

pub fn set_ksu_no_new_privs() -> anyhow::Result<()> {
    let result = ksuctl(
        ksu_uapi::KSU_IOCTL_DISABLE_ESCAPE_TO_ROOT,
        std::ptr::null_mut::<u8>(),
    )?;
    if result != 0 {
        bail!("unexpected result: {result}");
    }
    Ok(())
}

/// Get app profile from kernel by uid
pub fn get_app_profile(uid: i32) -> Result<ksu_uapi::app_profile> {
    let mut profile: ksu_uapi::app_profile = unsafe { std::mem::zeroed() };
    profile.version = ksu_uapi::KSU_APP_PROFILE_VER;
    profile.curr_uid = uid;
    let mut cmd = ksu_uapi::ksu_get_app_profile_cmd { profile };
    ksuctl(ksu_uapi::KSU_IOCTL_GET_APP_PROFILE, &raw mut cmd)?;
    Ok(cmd.profile)
}

/// Set app profile to kernel
pub fn set_app_profile(profile: &ksu_uapi::app_profile) -> Result<()> {
    let mut cmd = ksu_uapi::ksu_set_app_profile_cmd { profile: *profile };
    ksuctl(ksu_uapi::KSU_IOCTL_SET_APP_PROFILE, &raw mut cmd)?;
    Ok(())
}

/// Check if an app has root permission (allow_su)
pub fn is_app_granted(uid: i32) -> bool {
    match get_app_profile(uid) {
        Ok(profile) => profile.allow_su,
        Err(_) => false,
    }
}

/// Fill root_profile with default values matching APK Manager (Natives.kt Profile defaults)
/// Key: use_default=true, but selinux_domain="u:r:ksu:s0" and flags=NO_NEW_PRIVS must still be set
fn fill_default_root_profile(profile: &mut ksu_uapi::app_profile) {
    // Accessing union fields is unsafe in Rust
    unsafe {
        let rp = &mut profile.__bindgen_anon_1.rp_config;
        // APK default: rootUseDefault = true
        rp.use_default = true;
        // template_name stays empty (zeroed)
        // APK default: uid=0, gid=0
        rp.profile.uid = 0;
        rp.profile.gid = 0;
        // APK default: groups = empty list
        rp.profile.groups_count = 0;
        // groups already zeroed
        // APK default: capabilities = empty list -> effective=0, permitted/inheritable stay 0
        rp.profile.capabilities.effective = 0;
        rp.profile.capabilities.permitted = 0;
        rp.profile.capabilities.inheritable = 0;
        // APK default: context = "u:r:ksu:s0" (KERNEL_SU_DOMAIN)
        let domain = b"u:r:ksu:s0";
        for (i, &b) in domain.iter().enumerate() {
            if i < 63 {
                rp.profile.selinux_domain[i] = b as _;
            }
        }
        rp.profile.selinux_domain[domain.len().min(63)] = 0;
        // APK default: namespace = INHERITED (0)
        rp.profile.namespaces = 0;
        // APK default: flags = FLAG_KSU_NO_NEW_PRIVS (1)
        rp.profile.flags = 1;
    }
}

/// Grant root permission to an app
pub fn grant_app(uid: i32, package_name: &str) -> Result<()> {
    let mut profile = match get_app_profile(uid) {
        Ok(p) => p,
        Err(_) => {
            // Create new profile if not exists
            let mut p: ksu_uapi::app_profile = unsafe { std::mem::zeroed() };
            p.version = ksu_uapi::KSU_APP_PROFILE_VER;
            p.curr_uid = uid;
            // Copy package name to key (use `as _` for cross-platform char signedness)
            let name_bytes = package_name.as_bytes();
            let len = name_bytes.len().min(255);
            for i in 0..len {
                p.key[i] = name_bytes[i] as _;
            }
            p
        }
    };
    profile.allow_su = true;
    // Fill root_profile with APK Manager default values
    fill_default_root_profile(&mut profile);
    // Log detailed profile info for debugging
    unsafe {
        let rp = &profile.__bindgen_anon_1.rp_config;
        let domain_str: String = rp
            .profile
            .selinux_domain
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8 as char)
            .collect();
        log::info!(
            "grant_app profile: uid={uid} pkg={package_name} version={} curr_uid={} allow_su={} use_default={} rp_uid={} rp_gid={} groups_count={} caps_eff={} caps_perm={} caps_inh={} selinux_domain='{}' namespaces={} flags={}",
            profile.version,
            profile.curr_uid,
            profile.allow_su,
            rp.use_default,
            rp.profile.uid,
            rp.profile.gid,
            rp.profile.groups_count,
            rp.profile.capabilities.effective,
            rp.profile.capabilities.permitted,
            rp.profile.capabilities.inheritable,
            domain_str,
            rp.profile.namespaces,
            rp.profile.flags
        );
    }
    let result = set_app_profile(&profile);
    if let Err(ref e) = result {
        log::error!("grant_app failed for uid={uid} pkg={package_name}: {e}");
    } else {
        log::info!("grant_app succeeded for uid={uid} pkg={package_name}");
    }
    result
}

/// Revoke root permission from an app
pub fn revoke_app(uid: i32) -> Result<()> {
    if let Ok(mut profile) = get_app_profile(uid) {
        profile.allow_su = false;
        set_app_profile(&profile)?;
    }
    Ok(())
}

/// Default non-root profile UID (NOBODY_UID)
const DEFAULT_PROFILE_UID: i32 = 9999;

/// Get default "umount modules" setting for non-root apps
pub fn get_default_umount_modules() -> Result<bool> {
    match get_app_profile(DEFAULT_PROFILE_UID) {
        Ok(profile) => {
            // Access union field safely - nrp_config is used for non-root profiles
            let umount = unsafe { profile.__bindgen_anon_1.nrp_config.profile.umount_modules };
            Ok(umount)
        }
        Err(_) => Ok(true), // Default to true (enabled) if profile not found, matching stock KernelSU behavior
    }
}

/// Set default "umount modules" setting for non-root apps
pub fn set_default_umount_modules(enabled: bool) -> Result<()> {
    let mut profile = match get_app_profile(DEFAULT_PROFILE_UID) {
        Ok(p) => p,
        Err(_) => {
            // Create new default profile if not exists
            let mut p: ksu_uapi::app_profile = unsafe { std::mem::zeroed() };
            p.version = ksu_uapi::KSU_APP_PROFILE_VER;
            p.curr_uid = DEFAULT_PROFILE_UID;
            p.allow_su = false;
            // Set key to special marker for default profile (just "$")
            let key = "$";
            let key_bytes: Vec<i8> = key.bytes().map(|b| b as _).collect();
            let len = key_bytes.len().min(255);
            for i in 0..len {
                p.key[i] = key_bytes[i] as _;
            }
            p
        }
    };
    // Set union field - nrp_config for non-root profiles
    profile.__bindgen_anon_1.nrp_config.profile.umount_modules = enabled;
    set_app_profile(&profile)
}
