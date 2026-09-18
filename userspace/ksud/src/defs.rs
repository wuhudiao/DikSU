pub const ADB_DIR: &str = "/data/adb/";
pub const WORKING_DIR: &str = const_format::concatcp!(ADB_DIR, "ksu/");

/// The panel's three own state files.
///
/// Named outside the `android` module because the host build compiles the loader that reads them
/// too — it just never finds a file there.
pub const WEBUI_JUMPS_PATH: &str = const_format::concatcp!(WORKING_DIR, "webui_jumps.json");
pub const WEBUI_QUICK_RUN_PATH: &str = const_format::concatcp!(WORKING_DIR, "webui_quickrun.json");
pub const WEBUI_THEME_PATH: &str = const_format::concatcp!(WORKING_DIR, "webui_theme.json");

#[cfg(target_os = "android")]
mod android {
    use const_format::concatcp;

    use super::{ADB_DIR, WORKING_DIR};

    pub const BINARY_DIR: &str = concatcp!(WORKING_DIR, "bin/");
    pub const LIBRARY_DIR: &str = concatcp!(WORKING_DIR, "lib/");
    pub const LOG_DIR: &str = concatcp!(WORKING_DIR, "log/");
    pub const SULOGD_LOCK_PATH: &str = concatcp!(WORKING_DIR, "sulogd.lock");

    pub const PROFILE_DIR: &str = concatcp!(WORKING_DIR, "profile/");
    pub const PROFILE_SELINUX_DIR: &str = concatcp!(PROFILE_DIR, "selinux/");
    pub const PROFILE_TEMPLATE_DIR: &str = concatcp!(PROFILE_DIR, "templates/");

    pub const KSURC_PATH: &str = concatcp!(WORKING_DIR, ".ksurc");
    pub const WEBUI_PORT_PATH: &str = concatcp!(WORKING_DIR, "webui.port");
    pub const WEBUI_TOKEN_PATH: &str = concatcp!(WORKING_DIR, "webui.token");
    pub const DAEMON_PATH: &str = concatcp!(ADB_DIR, "ksud");
    pub const LIBADBROOT_PATH: &str = concatcp!(LIBRARY_DIR, "libadbroot.so");

    pub const DAEMON_LINK_PATH: &str = concatcp!(BINARY_DIR, "ksud");

    pub const MODULE_DIR: &str = concatcp!(ADB_DIR, "modules/");
    pub const MODULE_UPDATE_DIR: &str = concatcp!(ADB_DIR, "modules_update/");
    pub const METAMODULE_DIR: &str = concatcp!(ADB_DIR, "metamodule/");

    // Prefer /metadata/watchdog/ when present, else /metadata
    pub const PREINIT_DIR_WATCHDOG: &str = "/metadata/watchdog/ksu/";
    pub const PREINIT_DIR_DEFAULT: &str = "/metadata/ksu/";
    pub const MODULES_RC_FILE: &str = "modules.rc";
    pub const MODULES_RC_TMP_FILE: &str = ".modules.rc.tmp";

    pub const MODULE_WEB_DIR: &str = "webroot";
    pub const MODULE_ACTION_SH: &str = "action.sh";
    pub const DISABLE_FILE_NAME: &str = "disable";
    pub const UPDATE_FILE_NAME: &str = "update";
    pub const REMOVE_FILE_NAME: &str = "remove";
    pub const MODULE_INIT_RC_DIR: &str = "initrc";

    // Module config system
    pub const MODULE_CONFIG_DIR: &str = concatcp!(WORKING_DIR, "module_configs/");
    pub const PERSIST_CONFIG_NAME: &str = "persist.config";
    pub const TEMP_CONFIG_NAME: &str = "tmp.config";

    // Metamodule support
    pub const METAMODULE_MOUNT_SCRIPT: &str = "metamount.sh";
    pub const METAMODULE_METAINSTALL_SCRIPT: &str = "metainstall.sh";
    pub const METAMODULE_METAUNINSTALL_SCRIPT: &str = "metauninstall.sh";

    pub const KSU_BACKUP_DIR: &str = WORKING_DIR;
    pub const KSU_BACKUP_FILE_PREFIX: &str = "ksu_backup_";
    pub const BACKUP_FILENAME: &str = "stock_image.sha1";
    pub const KSU_TEMP_BACKUP_DIR_NAME: &str = "boot_backup";

    pub const DEFAULT_PACKAGE_NAME: &str = env!("KSU_PACKAGE_NAME");
}

/// The `sh` to hand a child.
///
/// On the device it is the ROM's own; on a host it is whatever `sh` the test environment has,
/// which is what lets the session and job code run under `cargo test`.
pub const SHELL_PATH: &str = if cfg!(target_os = "android") {
    "/system/bin/sh"
} else {
    "sh"
};

#[allow(unused)]
pub const VERSION_CODE: &str = env!("VERSION_CODE");
pub const VERSION_NAME: &str = env!("VERSION_NAME");
#[cfg(target_os = "android")]
pub const FULL_VERSION: &str = const_format::formatcp!(
    "{VERSION_NAME} (uapi: {})",
    crate::ksu_uapi::KERNEL_SU_UAPI_VERSION
);

#[cfg(target_os = "android")]
pub use android::*;
