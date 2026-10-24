//! Cross-process backing ownership and best-effort retirement.
use super::*;

const GATE: &str = "READERS";
const PIN: &str = ".pin";

/// Outcome of a cleanup pass. Pinned files remain eligible for a later pass.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PruneReport {
    /// Unreferenced object directories removed, including previously retired ones.
    pub removed_objects: usize,
    /// Unreferenced objects retained by live readers.
    pub pinned_objects: usize,
    /// Old immutable manifests removed.
    pub removed_manifests: usize,
    /// Retirement directories awaiting another deletion attempt.
    pub pending_objects: usize,
    /// A reader was acquiring its snapshot; retry on a later cleanup pass.
    pub acquisition_busy: bool,
    /// Nonfatal cleanup failures; publication is unaffected.
    pub errors: Vec<String>,
}

// Closing the last descriptor releases the lock. Do not explicitly unlock:
// a forked process can still own an inherited backing and its descriptor.
#[derive(Debug)]
struct ObjectPin {
    _file: File,
}

pub(super) fn create_pin(object: &Path) -> Result<()> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(object.join(PIN))?
        .sync_all()?;
    Ok(())
}

pub(super) fn pin_object(object: &Path) -> Result<Arc<dyn std::fmt::Debug + Send + Sync>> {
    let file = File::open(object.join(PIN)).context("Cannot open snapshot reader pin")?;
    FileExt::lock_shared(&file).context("Cannot acquire snapshot reader pin")?;
    Ok(Arc::new(ObjectPin { _file: file }))
}

pub(super) fn prepare_layout(directory: &Path) -> Result<()> {
    fs::create_dir_all(directory.join("trash"))?;
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(GATE))?
        .sync_all()?;
    Ok(())
}

pub(super) struct Gate(File);
impl Drop for Gate {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

pub(super) fn read_gate(directory: &Path) -> Result<Gate> {
    let file = match File::open(directory.join(GATE)) {
        Ok(file) => file,
        Err(error) => {
            // Protocol 2 predates the gate; retain its actionable upgrade error.
            if error.kind() == std::io::ErrorKind::NotFound {
                read_pointer(directory)?;
            }
            return Err(error).context("Cannot acquire snapshot reader gate");
        }
    };
    FileExt::lock_shared(&file).context("Cannot acquire snapshot reader gate")?;
    Ok(Gate(file))
}

fn generated_object(entry: &fs::DirEntry) -> Result<bool> {
    Ok(entry.file_type()?.is_dir()
        && entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.strip_prefix("object-").is_some_and(valid_id)))
}

fn lock_busy(error: &std::io::Error) -> bool {
    // Windows ERROR_LOCK_VIOLATION is not consistently mapped to WouldBlock
    // across supported Rust versions. Use fs2's platform-specific lock error.
    error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

impl SnapshotWriter {
    /// Enable reader-safe cleanup for a protocol-2 snapshot store.
    ///
    /// This is an **offline** upgrade: close every older process first. Older
    /// binaries cannot pin their files and must never access the upgraded store.
    /// The graph data is validated and retained without rewriting it.
    pub fn upgrade(root: impl AsRef<Path>) -> Result<Self> {
        let root = fs::canonicalize(root.as_ref())?;
        let writer_lock = acquire_writer_lock(&root)?;
        let directory = root.join(DIRECTORY);
        let head: Head = serde_json::from_slice(&fs::read(directory.join(CURRENT))?)?;
        if head.schema_version != 2 {
            bail!(
                "Offline upgrade requires reader protocol 2 (found {})",
                head.schema_version
            );
        }
        // Reuse normal manifest/path/checksum validation with the new schema.
        let reference = head.selected();
        let name = reference
            .manifest
            .strip_prefix("manifest-")
            .and_then(|name| name.strip_suffix(".json"));
        if !name.is_some_and(valid_id) {
            bail!("Invalid manifest name in CURRENT");
        }
        let bytes = fs::read(directory.join("manifests").join(&reference.manifest))?;
        if blake3::hash(&bytes).to_hex().as_str() != reference.manifest_hash {
            bail!("Selected manifest checksum mismatch");
        }
        let mut generation: Generation = serde_json::from_slice(&bytes)?;
        if generation.schema_version != 2 {
            bail!("Expected protocol-2 manifest");
        }
        generation.schema_version = SCHEMA_VERSION;
        generation.config.root = root;
        validate_record(&generation)?;
        let object = directory.join("objects").join(&generation.object_id);
        let path = object.join("store.r5tu");
        if fs::metadata(&path)?.len() != generation.object_length {
            bail!("Snapshot object length does not match its committed manifest");
        }
        let rdf = Snapshot::open(&path)?;
        validate_catalog(&generation, &rdf)?;
        drop(rdf);
        prepare_layout(&directory)?;
        if !object.join(PIN).try_exists()? {
            create_pin(&object)?;
        }
        sync_directory(&object)?;
        // No ancestry was recorded by protocol 2. Preserve its selected object
        // and start exact publication ancestry at this upgrade boundary.
        publish_manifest_with_hook(&directory, &generation, None, |_| Ok(()))?;
        let current = open_head(&directory)?;
        let mut writer = Self {
            directory,
            current,
            needs_reopen: false,
            _writer_lock: writer_lock,
        };
        writer.prune_best_effort();
        Ok(writer)
    }

    /// Remove unreferenced backing files, preserving current/previous head and
    /// every live view, including views held by independent processes.
    /// Failures leave garbage for the next writer startup, commit or prune.
    pub fn prune(&mut self) -> Result<PruneReport> {
        if self.needs_reopen {
            bail!("Reopen the writer before cleanup after a publication error");
        }
        let mut report = PruneReport::default();
        let gate = File::open(self.directory.join(GATE))?;
        match FileExt::try_lock_exclusive(&gate) {
            Ok(()) => {}
            Err(error) if lock_busy(&error) => {
                report.acquisition_busy = true;
                return Ok(report);
            }
            Err(error) => return Err(error.into()),
        }
        let gate = Gate(gate);
        let head = read_pointer(&self.directory)?;
        let mut roots = vec![head.selected()];
        roots.extend(head.previous);
        let mut objects = HashSet::new();
        let mut manifests = HashSet::new();
        for root in roots {
            let generation = read_manifest(&self.directory, &root)?;
            objects.insert(generation.object_id);
            manifests.insert(root.manifest);
        }
        let trash = self.directory.join("trash");
        for entry in fs::read_dir(self.directory.join("objects"))? {
            let entry = entry?;
            if !generated_object(&entry)? || objects.contains(entry.file_name().to_str().unwrap()) {
                continue;
            }
            let result = (|| -> Result<()> {
                // Orphans from interrupted writes/old protocol have no pin.
                // All published protocol-3 objects must already have one.
                let pin_path = entry.path().join(PIN);
                let pin = match File::open(&pin_path) {
                    Ok(pin) => Some(pin),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.into()),
                };
                if let Some(pin) = &pin {
                    match FileExt::try_lock_exclusive(pin) {
                        Ok(()) => {}
                        Err(error) if lock_busy(&error) => {
                            report.pinned_objects += 1;
                            return Ok(());
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                let destination = trash.join(entry.file_name());
                // Windows rename can replace a file with a directory. Never
                // overwrite an existing retirement entry, including symlinks.
                // Only this writer can create targets in the managed trash.
                match fs::symlink_metadata(&destination) {
                    Ok(_) => bail!(
                        "Retirement target already exists: {}",
                        destination.display()
                    ),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                // The exclusive acquisition gate prevents new owners. Once
                // exclusivity proves that no reader owns this object, close
                // the collector's handle before renaming its parent directory
                // (Windows rejects that rename while any child handle is open).
                drop(pin);
                fs::rename(entry.path(), destination)?;
                Ok(())
            })();
            if let Err(error) = result {
                report
                    .errors
                    .push(format!("{}: {error:#}", entry.path().display()));
            }
        }
        for entry in fs::read_dir(self.directory.join("manifests"))? {
            let entry = entry?;
            let name = entry.file_name();
            if !entry.file_type()?.is_file()
                || !name.to_str().is_some_and(|name| {
                    name.strip_prefix("manifest-")
                        .and_then(|name| name.strip_suffix(".json"))
                        .is_some_and(valid_id)
                        && !manifests.contains(name)
                })
            {
                continue;
            }
            match fs::remove_file(entry.path()) {
                Ok(()) => report.removed_manifests += 1,
                Err(error) => report
                    .errors
                    .push(format!("{}: {error}", entry.path().display())),
            }
        }
        drop(gate);
        // Retirement is permanent once renamed; no acquisition can find these.
        // Slow deletion/retries need not block readers acquiring the new head.
        for entry in fs::read_dir(trash)? {
            let entry = entry?;
            if !generated_object(&entry)? {
                continue;
            }
            match fs::remove_dir_all(entry.path()) {
                Ok(()) => report.removed_objects += 1,
                Err(error) => {
                    report.pending_objects += 1;
                    report
                        .errors
                        .push(format!("{}: {error}", entry.path().display()));
                }
            }
        }
        Ok(report)
    }

    pub(super) fn prune_best_effort(&mut self) {
        match self.prune() {
            Ok(report) if report.errors.is_empty() => {}
            Ok(report) => log::warn!("Snapshot cleanup deferred: {:?}", report.errors),
            Err(error) => log::warn!("Snapshot cleanup deferred: {error:#}"),
        }
    }
}
