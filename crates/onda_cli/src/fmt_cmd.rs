use same_file::Handle;
use std::{
    fs::{self, File, TryLockError},
    io::{self, Read, Seek, Write},
    path::Path,
};

pub(crate) fn run(input: &Path, check: bool) -> Result<(), String> {
    let error = |err: io::Error| format!("{}: {err}", input.display());
    if !fs::metadata(input).map_err(error)?.is_file() {
        return Err(format!("{}: expected a regular file", input.display()));
    }
    let mut original = File::open(input).map_err(error)?;
    let mut source = String::new();
    original.read_to_string(&mut source).map_err(error)?;
    let formatted = onda_frontend::format_source(&source, input).map_err(|diags| {
        crate::diag_print::format_diagnostics("could not format source", &diags)
    })?;
    if formatted == source {
        return Ok(());
    }
    if check {
        return Err(format!("{} needs formatting", input.display()));
    }

    write_if_unchanged(input, original, source.as_bytes(), formatted.as_bytes()).map_err(error)
}

fn write_if_unchanged(
    path: &Path,
    original: File,
    source: &[u8],
    formatted: &[u8],
) -> io::Result<()> {
    let original = Handle::from_file(original)?;
    let mut target = Handle::from_file(File::options().read(true).write(true).open(path)?)?;
    if original != target {
        return Err(file_changed());
    }
    target.as_file().try_lock().map_err(|error| match error {
        TryLockError::WouldBlock => io::Error::new(io::ErrorKind::WouldBlock, "file is locked"),
        TryLockError::Error(error) => error,
    })?;
    if target.as_file().metadata()?.permissions().readonly() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "file is read-only",
        ));
    }

    let mut current = Vec::with_capacity(source.len());
    target
        .as_file()
        .take(source.len() as u64 + 1)
        .read_to_end(&mut current)?;
    if current != source || target != Handle::from_path(path)? {
        return Err(file_changed());
    }

    let file = target.as_file_mut();
    file.rewind()?;
    file.write_all(formatted)?;
    if formatted.len() < source.len() {
        file.set_len(formatted.len() as u64)?;
    }
    file.sync_all()
}

fn file_changed() -> io::Error {
    io::Error::other("file changed during formatting")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_contents_changed_after_reading() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.onda");
        fs::write(&path, b"out1=1").unwrap();
        let original = File::open(&path).unwrap();
        fs::write(&path, b"out1=2").unwrap();

        let result = write_if_unchanged(&path, original, b"out1=1", b"out1 = 1");
        assert_eq!(
            result.unwrap_err().to_string(),
            "file changed during formatting"
        );
        assert_eq!(fs::read(&path).unwrap(), b"out1=2");
    }

    #[test]
    fn rejects_a_replaced_file_with_identical_contents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.onda");
        fs::write(&path, b"out1=1").unwrap();
        let original = File::open(&path).unwrap();
        fs::rename(&path, dir.path().join("previous.onda")).unwrap();
        fs::write(&path, b"out1=1").unwrap();

        let result = write_if_unchanged(&path, original, b"out1=1", b"out1 = 1");
        assert_eq!(
            result.unwrap_err().to_string(),
            "file changed during formatting"
        );
        assert_eq!(fs::read(&path).unwrap(), b"out1=1");
    }

    #[test]
    fn rejects_a_write_when_the_file_is_locked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.onda");
        fs::write(&path, b"out1=1").unwrap();
        let original = File::open(&path).unwrap();
        let editor = File::options().write(true).open(&path).unwrap();
        editor.try_lock().unwrap();

        let result = write_if_unchanged(&path, original, b"out1=1", b"out1 = 1");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::WouldBlock);
        drop(editor);
        assert_eq!(fs::read(&path).unwrap(), b"out1=1");
    }
}
