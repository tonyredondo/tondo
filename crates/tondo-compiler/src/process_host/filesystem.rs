//! Bounded filesystem observations. Staged values own their phase reservation;
//! no host identity is published until the complete result can enter the VM.

use super::*;

const MAX_DIRECTORY_ENTRIES: usize = 1_048_576;
const READ_CHUNK_BYTES: usize = 4096;
const NODE: u64 = tondo_vm::runtime::TEST_DETACHED_VALUE_BYTES;
const HOST_RECORD: u64 = tondo_vm::runtime::TEST_HOST_BUFFER_BYTES;
type AdmittedBuffer<T> = (T, Option<VmMemoryCharge>);

impl BootstrapHost {
    pub(super) fn invoke_filesystem_observation_admitted(
        &mut self,
        name: &str,
        arguments: &[RuntimeValue],
        response: &mut VmHostReturnBudget<'_>,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<RuntimeValue, VmError> {
        response.reserve(2 * NODE + 7, [])?;
        let [receiver] = arguments else {
            return Err(VmError::Host(
                "filesystem observation expects one receiver".into(),
            ));
        };
        let result = if name == "std.fs.Directory.list" {
            let base = self.directory_path(receiver)?;
            let bytes = native_path_bytes(base)?;
            let staged =
                self.stage_directory_paths(base, bytes, MAX_DIRECTORY_ENTRIES, response)?;
            self.publish_directory_paths(staged, admission)?
        } else {
            let length = match self.path(receiver) {
                Ok(path) => path.as_bytes().len(),
                Err(_) => return Ok(self.fs_result_error(FsError::InvalidPath)),
            };
            let maximum = length as u64 * if cfg!(unix) { 1 } else { 3 };
            // openDirectory retains this allocation; the other operations
            // release it after their private handle/iterator has closed.
            let mut path_memory = self.reserve_test_memory(maximum)?;
            let path = match self.filesystem_path(receiver) {
                Ok(path) => path,
                Err(error) => return Ok(self.fs_result_error(error)),
            };
            match name {
                "std.fs.openDirectory" => {
                    let mut record = self.reserve_buffer_payloads(std::iter::once(0))?;
                    self.admit_file_result(
                        admission,
                        VmHostReturnPreview::ResultOk(&RuntimeValue::Host {
                            kind: RuntimeHostValueKind::Directory,
                            id: self.next_value,
                        }),
                    )?;
                    match std::fs::symlink_metadata(&path) {
                        Ok(metadata) if metadata.is_dir() => {
                            if let Some(memory) = &mut path_memory {
                                memory.resize(path.capacity() as u64)?;
                            }
                            if let (Some(record), Some(mut path_memory)) =
                                (&mut record, path_memory)
                            {
                                let bytes = path_memory.bytes();
                                path_memory.transfer_to(record, bytes)?;
                            }
                            RuntimeValue::ResultOk(Box::new(self.publish_buffer(
                                RuntimeHostValueKind::Directory,
                                HostValue::Directory { path },
                                record,
                            )))
                        }
                        Ok(_) => self.fs_result_error(FsError::NotDirectory),
                        Err(error) => self.fs_io_result_error(&error),
                    }
                }
                "std.fs.readAll" => {
                    let success = self.next_io_bytes()?;
                    self.admit_file_result(admission, VmHostReturnPreview::ResultOk(&success))?;
                    let _file_memory = self.reserve_test_memory(HOST_RECORD)?;
                    match std::fs::File::open(path) {
                        Ok(mut file) => match self.read_bounded_file(&mut file)? {
                            Ok((bytes, memory)) => {
                                RuntimeValue::ResultOk(Box::new(self.publish_buffer(
                                    RuntimeHostValueKind::Bytes,
                                    HostValue::Bytes(bytes),
                                    memory,
                                )))
                            }
                            Err(error) => self.fs_result_error(error),
                        },
                        Err(error) => self.fs_io_result_error(&error),
                    }
                }
                "std.fs.list" => {
                    let staged = self.stage_directory_paths(
                        &path,
                        self.path(receiver)?.as_bytes(),
                        MAX_DIRECTORY_ENTRIES,
                        response,
                    )?;
                    self.publish_directory_paths(staged, admission)?
                }
                _ => return Err(VmError::Host("unknown filesystem observation".into())),
            }
        };
        let bytes = match &result {
            RuntimeValue::ResultErr(_) => 2 * NODE + 7,
            RuntimeValue::ResultOk(value) => match value.as_ref() {
                RuntimeValue::Array(paths) => (paths.len() as u64 + 2) * NODE,
                RuntimeValue::Host { .. } => 2 * NODE,
                _ => return Err(VmError::Invariant("unexpected filesystem result".into())),
            },
            _ => return Err(VmError::Invariant("filesystem result is not Result".into())),
        };
        response.shrink(bytes)?;
        Ok(result)
    }

    fn read_bounded_file(
        &self,
        reader: &mut impl Read,
    ) -> Result<Result<AdmittedBuffer<Vec<u8>>, FsError>, VmError> {
        let mut memory = self.reserve_buffer_payloads(std::iter::once(0))?;
        let _probe_memory = self.reserve_test_memory(1)?;
        let maximum = usize::try_from(self.max_bytes).unwrap_or(usize::MAX);
        let mut bytes = Vec::new();
        loop {
            let length = bytes.len();
            if length == maximum {
                // The one-byte probe distinguishes exact-limit EOF from an
                // oversized stream, including streams with no useful metadata.
                let mut probe = [0];
                match reader.read(&mut probe) {
                    Ok(0) => return Ok(Ok((bytes, memory))),
                    Ok(_) => return Ok(Err(FsError::ResourceLimit)),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Ok(Err(FsError::from_io(&error))),
                }
            }
            let next = length + READ_CHUNK_BYTES.min(maximum - length);
            if let Some(memory) = &mut memory {
                memory.resize(HOST_RECORD + next as u64)?;
            }
            if bytes.try_reserve_exact(next - length).is_err() {
                return Ok(Err(FsError::ResourceLimit));
            }
            bytes.resize(next, 0);
            match reader.read(&mut bytes[length..]) {
                Ok(0) => {
                    bytes.truncate(length);
                    return Ok(Ok((bytes, memory)));
                }
                Ok(count) => bytes.truncate(length + count),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => bytes.truncate(length),
                Err(error) => return Ok(Err(FsError::from_io(&error))),
            }
        }
    }

    fn stage_directory_paths(
        &self,
        base: &std::path::Path,
        base_bytes: &[u8],
        maximum_entries: usize,
        response: &mut VmHostReturnBudget<'_>,
    ) -> Result<Result<Vec<AdmittedBuffer<path::Path>>, FsError>, VmError> {
        let _iterator_memory = self.reserve_test_memory(HOST_RECORD)?;
        let entries = match std::fs::read_dir(base) {
            Ok(entries) => entries,
            Err(error) => return Ok(Err(FsError::from_io(&error))),
        };
        // One bounded native name is extracted at a time. Unlike collecting
        // DirEntry values, this never retains an unbounded OS-entry vector.
        let _name_memory =
            self.reserve_test_memory(path::MAX_PATH_BYTES as u64 * if cfg!(unix) { 1 } else { 3 })?;
        let mut paths: Vec<AdmittedBuffer<path::Path>> = Vec::new();
        let mut total = 0_u64;
        for entry in entries {
            if paths.len() == maximum_entries {
                return Ok(Err(FsError::ResourceLimit));
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => return Ok(Err(FsError::from_io(&error))),
            };
            let name = match native_file_name_bytes(&entry) {
                Ok(name) => name,
                Err(error) => return Ok(Err(error)),
            };
            let separator = usize::from(!base_bytes.is_empty() && !base_bytes.ends_with(b"/"));
            let length = base_bytes
                .len()
                .saturating_add(separator)
                .saturating_add(name.len());
            if length > path::MAX_PATH_BYTES || total.saturating_add(length as u64) > self.max_bytes
            {
                return Ok(Err(FsError::ResourceLimit));
            }
            total += length as u64;
            self.next_value
                .checked_add(paths.len() as u64 + 1)
                .ok_or(VmError::ResourceLimit {
                    resource: "host values",
                    limit: u64::MAX,
                })?;
            response
                .grow_before_construction(((paths.len() as u64 + 3) * NODE).max(2 * NODE + 7))?;
            let memory = self.reserve_buffer_payloads(std::iter::once(length))?;
            let mut child = Vec::with_capacity(length);
            child.extend_from_slice(base_bytes);
            if separator != 0 {
                child.push(b'/');
            }
            child.extend_from_slice(&name);
            let child = match path::Path::from_owned_bytes(child) {
                Ok(child) => child,
                Err(error) => {
                    return Ok(Err(match error {
                        path::PathError::ResourceLimit => FsError::ResourceLimit,
                        _ => FsError::InvalidPath,
                    }));
                }
            };
            paths.push((child, memory));
        }
        paths.sort_unstable_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
        Ok(Ok(paths))
    }

    fn publish_directory_paths(
        &mut self,
        staged: Result<Vec<AdmittedBuffer<path::Path>>, FsError>,
        admission: &mut VmHostImportAdmission<'_>,
    ) -> Result<RuntimeValue, VmError> {
        let paths = match staged {
            Ok(paths) => paths,
            Err(error) => return Ok(self.fs_result_error(error)),
        };
        let values = RuntimeValue::Array(
            (0..paths.len())
                .map(|index| RuntimeValue::Host {
                    kind: RuntimeHostValueKind::Path,
                    id: self.next_value + index as u64,
                })
                .collect(),
        );
        self.admit_file_result(admission, VmHostReturnPreview::ResultOk(&values))?;
        for (path, memory) in paths {
            self.publish_buffer(RuntimeHostValueKind::Path, HostValue::Path(path), memory);
        }
        Ok(RuntimeValue::ResultOk(Box::new(values)))
    }
}

fn native_path_bytes(path: &std::path::Path) -> Result<&[u8], VmError> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        Ok(path.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        path.to_str()
            .map(str::as_bytes)
            .ok_or_else(|| VmError::Host("Directory path encoding is invalid".into()))
    }
}

fn native_file_name_bytes(entry: &std::fs::DirEntry) -> Result<Vec<u8>, FsError> {
    let name = entry.file_name();
    #[cfg(unix)]
    {
        Ok(name.into_vec())
    }
    #[cfg(not(unix))]
    {
        name.into_string()
            .map(String::into_bytes)
            .map_err(|_| FsError::InvalidPath)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PartialReader<'a> {
        bytes: &'a [u8],
        consumed: usize,
        interrupt: bool,
        fail_at: Option<usize>,
    }

    impl Read for PartialReader<'_> {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if std::mem::take(&mut self.interrupt) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.fail_at.is_some_and(|limit| self.consumed >= limit) {
                return Err(io::ErrorKind::Other.into());
            }
            let count = output.len().min(2).min(self.bytes.len() - self.consumed);
            output[..count].copy_from_slice(&self.bytes[self.consumed..self.consumed + count]);
            self.consumed += count;
            Ok(count)
        }
    }

    #[test]
    fn filesystem_bounded_reads_handle_exact_eof_short_reads_and_errors() {
        for (input, limit, expected) in [
            (b"".as_slice(), 0, Ok(b"".as_slice())),
            (b"x".as_slice(), 0, Err(FsError::ResourceLimit)),
            (b"abc".as_slice(), 3, Ok(b"abc".as_slice())),
            (b"abcd".as_slice(), 3, Err(FsError::ResourceLimit)),
            (b"ab".as_slice(), 7, Ok(b"ab".as_slice())),
        ] {
            let mut host = BootstrapHost::with_max_bytes(Vec::new(), limit);
            let budget = VmMemoryBudget::new(65536);
            host.set_test_memory_budget(Some(budget.clone()));
            let mut reader = PartialReader {
                bytes: input,
                consumed: 0,
                interrupt: true,
                fail_at: None,
            };
            let result = host.read_bounded_file(&mut reader).unwrap();
            match expected {
                Ok(expected) => {
                    let (bytes, memory) = result.unwrap();
                    assert_eq!(bytes, expected);
                    assert_eq!(budget.live_bytes(), HOST_RECORD + bytes.capacity() as u64);
                    drop(memory);
                }
                Err(expected) => assert_eq!(result.unwrap_err(), expected),
            }
            assert!(reader.consumed as u64 <= limit + 1);
            assert_eq!(budget.live_bytes(), 0);
            assert_eq!(host.next_value, 0);
        }
        let mut host = BootstrapHost::default();
        let budget = VmMemoryBudget::new(4096);
        host.set_test_memory_budget(Some(budget.clone()));
        let mut reader = PartialReader {
            bytes: b"abc",
            consumed: 0,
            interrupt: false,
            fail_at: None,
        };
        assert!(
            host.read_bounded_file(&mut reader)
                .unwrap_err()
                .is_resource_limit()
        );
        assert_eq!(reader.consumed, 0, "read before buffer admission");
        assert_eq!(budget.live_bytes(), 0);
        let budget = VmMemoryBudget::new(16384);
        host.set_test_memory_budget(Some(budget.clone()));
        reader.fail_at = Some(2);
        assert_eq!(
            host.read_bounded_file(&mut reader).unwrap().unwrap_err(),
            FsError::Io
        );
        assert_eq!(reader.consumed, 2);
        assert_eq!(budget.live_bytes(), 0);
    }

    #[test]
    fn filesystem_listings_stage_native_order_and_reject_without_partial_handles() {
        let mut root = crate::test_temporaries::TemporaryRoot::create(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))
        .unwrap();
        for name in ["z", "a"] {
            std::fs::write(root.path().join(name), b"x").unwrap();
        }
        let base = path::Path::from_string(root.path().to_str().unwrap()).unwrap();
        let total = 2 * (base.as_bytes().len() + 2) as u64;
        for directory_method in [false, true] {
            for maximum in [total - 1, total] {
                let mut host = BootstrapHost::with_max_bytes(Vec::new(), maximum);
                let receiver = if directory_method {
                    host.allocate(
                        RuntimeHostValueKind::Directory,
                        HostValue::Directory {
                            path: root.path().to_owned(),
                        },
                    )
                } else {
                    host.allocate(RuntimeHostValueKind::Path, HostValue::Path(base.clone()))
                };
                let first = host.next_value;
                let budget = VmMemoryBudget::new(1024 * 1024);
                host.set_test_memory_budget(Some(budget.clone()));
                let returned = host
                    .invoke_owned(
                        if directory_method {
                            "std.fs.Directory.list"
                        } else {
                            "std.fs.list"
                        },
                        vec![receiver],
                        None,
                        Some(&budget),
                    )
                    .unwrap();
                if maximum < total {
                    assert_eq!(returned.value, host.fs_result_error(FsError::ResourceLimit));
                    assert_eq!(host.next_value, first);
                    assert!(host.buffer_memory.is_empty());
                } else {
                    let RuntimeValue::ResultOk(value) = &returned.value else {
                        panic!("missing success")
                    };
                    let RuntimeValue::Array(values) = value.as_ref() else {
                        panic!("missing paths")
                    };
                    assert_eq!(values.len(), 2);
                    assert!(host.path(&values[0]).unwrap().as_bytes().ends_with(b"/a"));
                    assert!(host.path(&values[1]).unwrap().as_bytes().ends_with(b"/z"));
                    assert_eq!(host.next_value, first + 2);
                    assert_eq!(budget.live_bytes(), total + 2 * HOST_RECORD + 4 * NODE);
                }
                drop(returned);
                host.collect_host_values(&tondo_vm::runtime::VmHostRoots::new())
                    .unwrap();
                assert_eq!(budget.live_bytes(), 0);
            }
        }
        let mut host = BootstrapHost::default();
        let budget = VmMemoryBudget::new(1024 * 1024);
        host.set_test_memory_budget(Some(budget.clone()));
        let mut response = VmHostReturnBudget::new(Some(&budget));
        response.reserve(2 * NODE + 7, []).unwrap();
        let result = host
            .stage_directory_paths(root.path(), base.as_bytes(), 1, &mut response)
            .unwrap();
        assert_eq!(result.unwrap_err(), FsError::ResourceLimit);
        assert_eq!(host.next_value, 0);
        assert!(host.values.is_empty());
        drop(response);
        assert_eq!(budget.live_bytes(), 0);
        root.cleanup().unwrap();
    }

    #[test]
    fn filesystem_directory_path_storage_follows_handle_collection() {
        let mut root = crate::test_temporaries::TemporaryRoot::create(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))
        .unwrap();
        let mut host = BootstrapHost::default();
        let receiver = host.allocate(
            RuntimeHostValueKind::Path,
            HostValue::Path(path::Path::from_string(root.path().to_str().unwrap()).unwrap()),
        );
        let budget = VmMemoryBudget::new(65536);
        host.set_test_memory_budget(Some(budget.clone()));
        let returned = host
            .invoke_owned("std.fs.openDirectory", vec![receiver], None, Some(&budget))
            .unwrap();
        let RuntimeValue::ResultOk(directory) = &returned.value else {
            panic!("missing directory")
        };
        let mut roots = tondo_vm::runtime::VmHostRoots::new();
        directory.trace_host_roots(&mut roots);
        host.collect_host_values(&roots).unwrap();
        assert_eq!(host.directory_path(directory).unwrap(), root.path());
        assert!(
            budget.live_bytes() >= HOST_RECORD + root.path().as_os_str().len() as u64 + 2 * NODE
        );
        host.cleanup(directory).unwrap();
        assert_eq!(budget.live_bytes(), 2 * NODE);
        drop(returned);
        assert_eq!(budget.live_bytes(), 0);
        root.cleanup().unwrap();
    }

    #[test]
    fn filesystem_observation_results_are_admitted_before_handle_publication() {
        use crate::test_control::EnvelopeLimits;
        use tondo_vm::runtime::{VmLimits, execute_with_limits};
        let mut root = crate::test_temporaries::TemporaryRoot::create(std::path::Path::new(env!(
            "CARGO_MANIFEST_DIR"
        )))
        .unwrap();
        std::fs::write(root.path().join("input"), b"contents").unwrap();
        let directory = serde_json::to_string(root.path().to_str().unwrap()).unwrap();
        let file = serde_json::to_string(root.path().join("input").to_str().unwrap()).unwrap();
        for body in [
            format!(
                "let location = path.Path.fromString({file})?\nlet data = fs.readAll(location)?\nassert(data.length() == 8)"
            ),
            format!(
                "let location = path.Path.fromString({directory})?\nlet entries = fs.list(location)?\nassert(entries.length() == 1)"
            ),
            format!(
                "let location = path.Path.fromString({directory})?\nvar directory = fs.openDirectory(location)?\nlet entries = directory.list()?\nassert(entries.length() == 1)"
            ),
        ] {
            let source = format!(
                "import std.fs\nimport std.path\nimport std.bytes\ntest observation {{\n{body}\n}}\n"
            );
            let (program, entry) = super::super::tests::compile_host_admission_source(
                &source,
                crate::driver::BuildTarget::vm_hosted_capabilities(),
                crate::driver::Operation::Test,
            );
            let mut succeeded = 0;
            let mut rejected = 0;
            for memory in [4096, 8192, 16384, 32768, 65536, 131072, 262144] {
                let mut host = BootstrapHost::default();
                let participation = TestParticipation::new(
                    EnvelopeLimits::new(65536, 65536, 65536),
                    BTreeMap::new(),
                    false,
                );
                host.install_testing_participation(participation.clone());
                execute_with_limits(
                    &program,
                    entry,
                    &mut host,
                    VmLimits {
                        max_heap_bytes: memory,
                        ..VmLimits::default()
                    },
                )
                .unwrap();
                let executions = participation.executions().unwrap();
                assert_eq!(executions.len(), 1);
                let terminal = executions[0].report.terminal();
                if terminal.is_none() {
                    succeeded += 1;
                } else {
                    rejected += 1;
                    assert!(
                        matches!(
                            terminal,
                            Some(crate::test_control::Terminal::ResourceLimit { kind: "memory" })
                        ),
                        "{body}/{memory}: {terminal:?}"
                    );
                }
                assert!(host.values.is_empty());
                assert!(host.buffer_memory.is_empty());
                assert!(host.ready_jobs.is_empty());
                assert!(host.async_memory.is_empty());
                let owner = host.test_memory.clone().unwrap();
                drop(host);
                assert_eq!(owner.live_bytes(), 0);
            }
            assert!(
                succeeded > 0 && rejected > 0,
                "{body}: missing budget transition"
            );
        }
        root.cleanup().unwrap();
    }
}
