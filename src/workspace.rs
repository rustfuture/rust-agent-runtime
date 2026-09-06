use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

pub struct WorkspaceEditor {
    root: PathBuf,
    max_file_bytes: usize,
}

impl WorkspaceEditor {
    pub fn new(root: &Path, max_file_bytes: usize) -> io::Result<Self> {
        if max_file_bytes == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file limit must be positive",
            ));
        }
        Ok(Self {
            root: root.canonicalize()?,
            max_file_bytes,
        })
    }

    pub fn read(&self, relative: &str) -> io::Result<String> {
        let path = self.resolve_existing(relative)?;
        let metadata = fs::metadata(&path)?;
        if !metadata.is_file() || metadata.len() as usize > self.max_file_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "file is not a bounded regular file",
            ));
        }
        fs::read_to_string(path)
    }

    pub fn replace_once(
        &self,
        relative: &str,
        expected: &str,
        replacement: &str,
    ) -> io::Result<()> {
        if expected.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected text cannot be empty",
            ));
        }
        let path = self.resolve_existing(relative)?;
        let content = self.read(relative)?;
        if content.match_indices(expected).count() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected text must occur exactly once",
            ));
        }
        let updated = content.replacen(expected, replacement, 1);
        if updated.len() > self.max_file_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "updated file exceeds size limit",
            ));
        }
        let temp = path.with_extension(format!("agent-tmp-{}", std::process::id()));
        let permissions = fs::metadata(&path)?.permissions();
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        file.write_all(updated.as_bytes())?;
        file.sync_all()?;
        fs::set_permissions(&temp, permissions)?;
        fs::rename(&temp, &path)?;
        Ok(())
    }

    fn resolve_existing(&self, relative: &str) -> io::Result<PathBuf> {
        let relative = Path::new(relative);
        if relative.as_os_str().is_empty()
            || relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "path is not a plain relative path",
            ));
        }
        let path = self.root.join(relative).canonicalize()?;
        if !path.starts_with(&self.root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "path escapes workspace",
            ));
        }
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("workspace-editor-{}-{name}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn replaces_one_exact_match() {
        let root = workspace("replace");
        fs::write(root.join("value.txt"), "before\n").unwrap();
        let editor = WorkspaceEditor::new(&root, 1024).unwrap();
        editor.replace_once("value.txt", "before", "after").unwrap();
        assert_eq!(editor.read("value.txt").unwrap(), "after\n");
        assert!(editor.replace_once("value.txt", "missing", "x").is_err());
    }

    #[test]
    fn rejects_parent_absolute_and_symlink_escape() {
        let root = workspace("escape");
        fs::write(root.join("inside.txt"), "ok").unwrap();
        let editor = WorkspaceEditor::new(&root, 1024).unwrap();
        assert_eq!(
            editor.read("../outside.txt").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            editor.read("/etc/hosts").unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/hosts", root.join("link")).unwrap();
            assert_eq!(
                editor.read("link").unwrap_err().kind(),
                io::ErrorKind::PermissionDenied
            );
        }
    }
}
