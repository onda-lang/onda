#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::env;
use std::path::{Path, PathBuf};
use std::process::{self, Command};

#[cfg(target_os = "windows")]
const ONDA_CLI_FILE_NAME: &str = "onda.exe";
#[cfg(not(target_os = "windows"))]
const ONDA_CLI_FILE_NAME: &str = "onda";

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    if let Err(error) = launch_onda_run() {
        show_launch_error(&error);
        process::exit(1);
    }
}

fn launch_onda_run() -> Result<(), String> {
    let launcher_path =
        env::current_exe().map_err(|error| format!("Could not locate Onda Run: {error}"))?;
    let onda_cli = find_onda_cli(&launcher_path).ok_or_else(|| {
        format!("Could not find {ONDA_CLI_FILE_NAME} next to Onda Run or in its bin folder.")
    })?;

    let mut command = Command::new(&onda_cli);
    command.arg("run").args(env::args_os().skip(1));

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let status = command
        .status()
        .map_err(|error| format!("Could not start {}: {error}", onda_cli.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Onda Run exited with {status}."))
    }
}

fn find_onda_cli(launcher_path: &Path) -> Option<PathBuf> {
    let launcher_dir = launcher_path.parent()?;
    [
        launcher_dir.join("bin").join(ONDA_CLI_FILE_NAME),
        launcher_dir.join(ONDA_CLI_FILE_NAME),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

#[cfg(target_os = "windows")]
fn show_launch_error(message: &str) {
    use std::ffi::{c_void, OsStr};
    use std::os::windows::ffi::OsStrExt as _;

    const MB_OK: u32 = 0;
    const MB_ICONERROR: u32 = 0x10;

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(
            window: *mut c_void,
            text: *const u16,
            caption: *const u16,
            message_type: u32,
        ) -> i32;
    }

    fn null_terminated_wide(value: &str) -> Vec<u16> {
        OsStr::new(value)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let text = null_terminated_wide(message);
    let caption = null_terminated_wide("Onda Run");
    // SAFETY: Both strings are valid, null-terminated UTF-16 buffers and the
    // null window handle makes this an application-modal error dialog.
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn show_launch_error(message: &str) {
    eprintln!("{message}");
}

#[cfg(test)]
mod tests {
    use super::{find_onda_cli, ONDA_CLI_FILE_NAME};
    use std::fs;

    #[test]
    fn cli_lookup_supports_release_bundles_and_local_builds() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock should follow the Unix epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("onda-run-launcher-{}-{unique}", std::process::id()));
        let launcher = root.join("Onda.exe");
        let bundled_cli = root.join("bin").join(ONDA_CLI_FILE_NAME);
        let sibling_cli = root.join(ONDA_CLI_FILE_NAME);

        fs::create_dir_all(
            bundled_cli
                .parent()
                .expect("bundled CLI should have a parent"),
        )
        .expect("temporary bundle should be created");
        fs::write(&launcher, []).expect("launcher fixture should be created");
        fs::write(&sibling_cli, []).expect("sibling CLI fixture should be created");
        fs::write(&bundled_cli, []).expect("bundled CLI fixture should be created");

        assert_eq!(
            find_onda_cli(&launcher).as_deref(),
            Some(bundled_cli.as_path())
        );
        fs::remove_file(&bundled_cli).expect("bundled CLI fixture should be removable");
        assert_eq!(
            find_onda_cli(&launcher).as_deref(),
            Some(sibling_cli.as_path())
        );

        fs::remove_dir_all(root).expect("temporary bundle should be removable");
    }
}
