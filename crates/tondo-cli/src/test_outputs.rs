//! Rollback of requested final outputs until the invocation has committed.
//! Backups remain on disk so an existing report does not consume VM budgets
//! or unbounded coordinator memory. Content-addressed artifact blobs are not
//! final outputs and may remain unreferenced after an interrupted invocation.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static BACKUP_SERIAL: AtomicU64 = AtomicU64::new(0);

pub struct OutputTransaction {
    backups: Vec<(PathBuf, Option<PathBuf>)>,
    committed: bool,
}

impl OutputTransaction {
    pub fn capture(paths: impl IntoIterator<Item = PathBuf>) -> io::Result<Self> {
        let mut transaction = Self {
            backups: Vec::new(),
            committed: false,
        };
        for path in paths.into_iter().collect::<BTreeSet<_>>() {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    let backup = path.with_file_name(format!(
                        ".tondo-output-backup-{}-{}",
                        std::process::id(),
                        BACKUP_SERIAL.fetch_add(1, Ordering::Relaxed)
                    ));
                    let mut target = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&backup)?;
                    let copied = File::open(&path)
                        .and_then(|mut source| io::copy(&mut source, &mut target))
                        .and_then(|_| target.set_permissions(metadata.permissions()));
                    if let Err(error) = copied {
                        drop(target);
                        let _ = fs::remove_file(&backup);
                        return Err(error);
                    }
                    transaction.backups.push((path.clone(), Some(backup)));
                }
                Ok(_) => {
                    return Err(io::Error::other(format!(
                        "final output is not a regular file: {}",
                        path.display()
                    )));
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    transaction.backups.push((path, None))
                }
                Err(error) => return Err(error),
            }
        }
        Ok(transaction)
    }

    pub fn commit(mut self) -> io::Result<()> {
        self.committed = true;
        for (_, backup) in &self.backups {
            if let Some(backup) = backup {
                fs::remove_file(backup)?;
            }
        }
        Ok(())
    }

    pub fn rollback(&mut self) -> io::Result<()> {
        let mut failure = None;
        for (path, backup) in self.backups.drain(..).rev() {
            let result = match backup {
                Some(backup) => fs::rename(backup, &path),
                None => match fs::remove_file(&path) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    other => other,
                },
            };
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for OutputTransaction {
    fn drop(&mut self) {
        if !self.committed
            && let Err(error) = self.rollback()
        {
            super::test_interrupt::isolation_lost();
            eprintln!("tondo test: cannot restore final output: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "tondo-output-transaction-{}-{}",
            std::process::id(),
            BACKUP_SERIAL.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn aborted_publication_restores_all_output_kinds_and_absence() {
        let root = directory();
        let paths = [
            "report.json",
            "report.xml",
            "manifest.json",
            "snapshots.json",
        ]
        .map(|name| root.join(name));
        for path in &paths[..3] {
            fs::write(path, b"previous complete bytes").unwrap();
        }
        {
            let _transaction = OutputTransaction::capture(paths.clone()).unwrap();
            for path in &paths {
                fs::write(path, b"new incomplete bytes").unwrap();
            }
        }
        for path in &paths[..3] {
            assert_eq!(fs::read(path).unwrap(), b"previous complete bytes");
        }
        assert!(!paths[3].exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
        let transaction = OutputTransaction::capture(paths.clone()).unwrap();
        for path in &paths {
            fs::write(path, b"new complete bytes").unwrap();
        }
        transaction.commit().unwrap();
        for path in &paths {
            assert_eq!(fs::read(path).unwrap(), b"new complete bytes");
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), 4);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_destination_and_failed_restoration_are_explicit() {
        let root = directory();
        let previous = root.join("a-report.json");
        fs::write(&previous, b"prior").unwrap();
        let directory = root.join("b-directory");
        fs::create_dir(&directory).unwrap();
        assert!(OutputTransaction::capture([previous.clone(), directory.clone()]).is_err());
        assert_eq!(fs::read(&previous).unwrap(), b"prior");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        let missing = root.join("missing.json");
        let mut transaction = OutputTransaction::capture([missing.clone()]).unwrap();
        fs::create_dir(&missing).unwrap();
        assert!(transaction.rollback().is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
