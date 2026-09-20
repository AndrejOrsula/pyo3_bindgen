#[cfg(windows)]
use pyo3_bindgen_engine::TextArtifact;
use pyo3_bindgen_engine::{
    stable_hash, ArtifactDurability, ArtifactStatus, Codegen, Config, ModuleIr, OutputMetadata,
    RustArtifact, SchemaIr,
};
use quote::quote;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(windows)]
use std::path::Path;

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_dir(test_name: &str) -> PathBuf {
    for _ in 0..128 {
        let path = std::env::temp_dir().join(format!(
            "pyo3_bindgen_engine_{test_name}_{}_{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("cannot create exclusive artifact fixture: {error}"),
        }
    }
    panic!("cannot allocate an unused artifact fixture directory");
}

fn empty_schema() -> SchemaIr {
    SchemaIr {
        modules: vec![ModuleIr {
            name: "example".to_owned(),
            docstring: None,
            is_package: false,
            classes: vec![],
            enums: vec![],
            functions: vec![],
            properties: vec![],
            submodules: vec![],
            prelude: vec![],
            imports: vec![],
            type_vars: vec![],
        }],
    }
}

fn codegen() -> Codegen {
    Codegen::new(
        Config::builder()
            .generate_provenance(false)
            .generate_preludes(false)
            .build(),
    )
    .modules_from_ir(empty_schema())
    .unwrap()
}

#[test]
fn rust_artifact_formats_hashes_and_checks_generated_code() {
    let artifact = RustArtifact::from_tokens(quote! {
        pub fn answer()->i32{42}
    })
    .unwrap();

    assert_eq!(artifact.contents(), "pub fn answer() -> i32 {\n    42\n}\n");
    assert_eq!(artifact.hash(), stable_hash(artifact.bytes()));

    let directory = temp_dir("artifact_check");
    let output = directory.join("nested/bindings.rs");
    artifact.write(&output).unwrap();
    assert_eq!(artifact.check(&output).unwrap(), ArtifactStatus::Current);

    std::fs::write(&output, "stale").unwrap();
    assert_eq!(artifact.check(&output).unwrap(), ArtifactStatus::Drifted);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn rust_artifact_detailed_write_reports_publication_durability() {
    let directory = temp_dir("artifact_detailed_success");
    let output = directory.join("bindings.rs");
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();

    let receipt = artifact.write_detailed(&output).unwrap();

    #[cfg(unix)]
    assert!(matches!(
        receipt.durability,
        ArtifactDurability::DirectorySynchronized
    ));
    #[cfg(not(unix))]
    assert!(matches!(
        receipt.durability,
        ArtifactDurability::PlatformUnsupported
    ));
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        artifact.contents()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn rust_artifact_rejects_missing_final_name_before_creating_parent() {
    let directory = temp_dir("artifact_missing_final_name");
    let rejected_parent = directory.join("created");
    let output = rejected_parent.join("..");
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();

    let error = artifact.write(&output).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("has no file name"));
    assert!(
        !rejected_parent.exists(),
        "filename validation must run before parent creation"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(any(unix, windows))]
#[test]
fn rust_artifact_writes_near_component_limit_destination_without_staging_leaks() {
    let directory = temp_dir("artifact_near_component_limit");
    // Windows canonicalization yields a verbatim path, avoiding the legacy
    // `MAX_PATH` limit while this test targets the independent component limit.
    let output = std::fs::canonicalize(&directory)
        .unwrap()
        .join(format!("{}.rs", "a".repeat(240)));
    RustArtifact::from_tokens(quote!(
        pub fn initial() {}
    ))
    .unwrap()
    .write(&output)
    .unwrap();
    let replacement = RustArtifact::from_tokens(quote!(
        pub fn replacement() {}
    ))
    .unwrap();

    replacement.write(&output).unwrap();

    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        replacement.contents()
    );
    assert_eq!(
        std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        [output.file_name().unwrap()]
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(windows)]
#[test]
fn direct_artifact_writers_leave_reserved_parent_components_to_windows() {
    let directory = temp_dir("artifact_windows_reserved_components");
    let rust_artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();
    let text_artifact = TextArtifact::new("generated report");

    // Keep the reserved spelling in an intermediate component. Passing a final NUL/CON-like
    // name to the writer could target a device, so this test observes only the non-destructive
    // parent-directory creation failure. The direct API intentionally leaves this lexical policy
    // to Windows instead of claiming the publication-session validator's rejection contract.
    for reserved_parent in ["CON", "NUL.txt", "com1", "LPT9"] {
        let parent = directory.join(reserved_parent);
        let rust_error = rust_artifact.write(parent.join("bindings.rs")).unwrap_err();
        assert!(
            rust_error.raw_os_error().is_some(),
            "Windows must reject reserved parent component {reserved_parent:?} through the OS, got {rust_error:?}"
        );
        let text_error = text_artifact.write(parent.join("report.json")).unwrap_err();
        assert!(
            text_error.raw_os_error().is_some(),
            "Windows must reject reserved parent component {reserved_parent:?} through the OS, got {text_error:?}"
        );
    }

    assert!(
        std::fs::read_dir(&directory).unwrap().next().is_none(),
        "reserved parent probes must not create a directory or staged artifact"
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(windows)]
#[test]
fn direct_artifact_writers_publish_to_a_regular_temp_path() {
    let directory = temp_dir("artifact_windows_temp_path");
    let rust_output = directory.join("bindings.rs");
    let text_output = directory.join("report.json");
    let rust_artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();
    let text_artifact = TextArtifact::new("generated report");

    rust_artifact.write(&rust_output).unwrap();
    text_artifact.write(&text_output).unwrap();

    assert_eq!(
        std::fs::read_to_string(rust_output).unwrap(),
        rust_artifact.contents()
    );
    assert_eq!(
        std::fs::read_to_string(text_output).unwrap(),
        text_artifact.contents()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(windows)]
struct OwnedArtifactDirectory {
    path: PathBuf,
}

#[cfg(windows)]
impl OwnedArtifactDirectory {
    fn new(test_name: &str) -> Self {
        Self {
            path: temp_dir(test_name),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(windows)]
impl Drop for OwnedArtifactDirectory {
    fn drop(&mut self) {
        // `temp_dir` created this child exclusively; never remove a caller-owned path.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(windows)]
fn windows_test_path(path: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    let mut encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
    assert!(
        !encoded.contains(&0),
        "Windows test paths must not contain NULs"
    );
    encoded.push(0);
    encoded
}

#[cfg(windows)]
fn file_security_descriptor(path: &Path, information: u32) -> Vec<usize> {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::Security::GetFileSecurityW;

    let encoded = windows_test_path(path);
    let mut bytes_needed = 0;
    let _ = unsafe {
        // SAFETY: `encoded` is a live, NUL-terminated UTF-16 path and the null
        // descriptor with zero length is the documented size probe.
        GetFileSecurityW(
            encoded.as_ptr(),
            information,
            ptr::null_mut(),
            0,
            &mut bytes_needed,
        )
    };
    assert!(
        bytes_needed > 0,
        "GetFileSecurityW did not report a descriptor size: {:?}",
        std::io::Error::last_os_error()
    );

    let mut descriptor = vec![0usize; (bytes_needed as usize).div_ceil(size_of::<usize>())];
    let loaded = unsafe {
        // SAFETY: `descriptor` has at least `bytes_needed` writable bytes and
        // all pointers remain valid for this synchronous API call.
        GetFileSecurityW(
            encoded.as_ptr(),
            information,
            descriptor.as_mut_ptr().cast(),
            bytes_needed,
            &mut bytes_needed,
        )
    };
    assert_ne!(
        loaded,
        0,
        "GetFileSecurityW failed: {:?}",
        std::io::Error::last_os_error()
    );

    descriptor
}

#[cfg(windows)]
fn file_owner_sid(path: &Path) -> Vec<u32> {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::Security::{
        GetLengthSid, GetSecurityDescriptorOwner, OWNER_SECURITY_INFORMATION,
    };

    let mut descriptor = file_security_descriptor(path, OWNER_SECURITY_INFORMATION);
    let mut owner_defaulted = 0;
    let mut owner = ptr::null_mut();
    let queried = unsafe {
        // SAFETY: the descriptor was filled by `GetFileSecurityW`; the output
        // pointers are valid writable locals for this synchronous call.
        GetSecurityDescriptorOwner(
            descriptor.as_mut_ptr().cast(),
            &mut owner,
            &mut owner_defaulted,
        )
    };
    assert_ne!(
        queried,
        0,
        "GetSecurityDescriptorOwner failed: {:?}",
        std::io::Error::last_os_error()
    );
    assert!(!owner.is_null(), "destination owner SID must be present");

    let sid_size = unsafe {
        // SAFETY: `owner` is non-null and points into the live descriptor
        // returned by `GetFileSecurityW`.
        usize::try_from(GetLengthSid(owner)).expect("owner SID length must fit usize")
    };
    let mut owner_storage = vec![0u32; sid_size.div_ceil(size_of::<u32>())];
    unsafe {
        // SAFETY: both SID buffers are valid for `sid_size` bytes, and the
        // destination has enough aligned storage for the copied SID.
        ptr::copy_nonoverlapping(
            owner.cast::<u8>(),
            owner_storage.as_mut_ptr().cast::<u8>(),
            sid_size,
        );
    }
    owner_storage
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct FileDacl {
    protected: bool,
    aces: Vec<Vec<u8>>,
}

#[cfg(windows)]
fn file_dacl(path: &Path) -> FileDacl {
    use std::{mem::size_of, ptr, slice};
    use windows_sys::Win32::Security::{
        GetAce, GetLengthSid, GetSecurityDescriptorControl, GetSecurityDescriptorDacl, ACE_HEADER,
        ACL, DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
    };

    let mut descriptor = file_security_descriptor(path, DACL_SECURITY_INFORMATION);

    let mut dacl_present = 0;
    let mut dacl_defaulted = 0;
    let mut dacl = ptr::null_mut::<ACL>();
    let queried = unsafe {
        // SAFETY: the descriptor was filled by `GetFileSecurityW`; the output
        // pointers are valid writable locals for this synchronous call.
        GetSecurityDescriptorDacl(
            descriptor.as_mut_ptr().cast(),
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        )
    };
    assert_ne!(
        queried,
        0,
        "GetSecurityDescriptorDacl failed: {:?}",
        std::io::Error::last_os_error()
    );
    assert_ne!(dacl_present, 0, "destination must have a DACL");
    assert!(!dacl.is_null(), "destination DACL pointer must be present");

    let mut control = 0;
    let mut revision = 0;
    let control_queried = unsafe {
        // SAFETY: the descriptor was filled by `GetFileSecurityW`; the output
        // pointers are valid writable locals for this synchronous call.
        GetSecurityDescriptorControl(descriptor.as_mut_ptr().cast(), &mut control, &mut revision)
    };
    assert_ne!(
        control_queried,
        0,
        "GetSecurityDescriptorControl failed: {:?}",
        std::io::Error::last_os_error()
    );

    let ace_count = unsafe {
        // SAFETY: `dacl` is non-null and was returned by
        // `GetSecurityDescriptorDacl` for the live descriptor above.
        (*dacl).AceCount
    };
    let mut aces = Vec::with_capacity(ace_count as usize);
    for index in 0..ace_count as u32 {
        let mut ace = ptr::null_mut();
        let found = unsafe {
            // SAFETY: `dacl` is the live ACL returned above, `index` is bounded
            // by its ACE count, and `ace` is a valid output pointer.
            GetAce(dacl, index, &mut ace)
        };
        assert_ne!(
            found,
            0,
            "GetAce failed: {:?}",
            std::io::Error::last_os_error()
        );
        assert!(!ace.is_null(), "ACL ACE pointer must be present");
        let ace_size = unsafe {
            // SAFETY: `ace` is a non-null ACE returned by `GetAce`.
            (*ace.cast::<ACE_HEADER>()).AceSize as usize
        };
        assert!(ace_size >= size_of::<ACE_HEADER>(), "ACL ACE is truncated");
        let ace_type = unsafe {
            // SAFETY: `ace` is a non-null ACE returned by `GetAce`.
            (*ace.cast::<ACE_HEADER>()).AceType
        };
        let meaningful_size = match ace_type {
            // These ACE forms end in a variable-length SID. The SID length is
            // always a multiple of four, so any bytes after this size are ACL
            // alignment padding and must not participate in the comparison.
            0..=3 | 17 => {
                let sid_offset = size_of::<ACE_HEADER>() + size_of::<u32>();
                assert!(ace_size >= sid_offset, "ACL ACE SID is truncated");
                let sid = unsafe {
                    // SAFETY: `sid_offset` is within this ACE, and the pointer
                    // remains valid for this synchronous `GetLengthSid` call.
                    ace.cast::<u8>().add(sid_offset).cast()
                };
                let sid_size = unsafe {
                    // SAFETY: the OS supplied this SID-bearing ACE through
                    // `GetAce`; `sid` points at its SID payload.
                    usize::try_from(GetLengthSid(sid)).expect("ACE SID length must fit usize")
                };
                assert!(sid_size > 0, "ACL ACE SID must have a non-zero size");
                sid_offset + sid_size
            }
            _ => ace_size,
        };
        assert!(
            meaningful_size <= ace_size,
            "ACL ACE meaningful bytes exceed its reported size"
        );
        aces.push(unsafe {
            // SAFETY: the API-reported ACE size covers the live ACE returned by
            // `GetAce`; `meaningful_size` excludes only trailing alignment
            // padding, and the copied bytes outlive the descriptor allocation.
            slice::from_raw_parts(ace.cast::<u8>(), meaningful_size).to_vec()
        });
    }
    FileDacl {
        protected: control & SE_DACL_PROTECTED != 0,
        aces,
    }
}

#[cfg(windows)]
fn set_nondefault_dacl(path: &Path) -> FileDacl {
    use std::mem::size_of;
    use windows_sys::Win32::{
        Security::{
            AddAccessAllowedAce, GetLengthSid, InitializeAcl, InitializeSecurityDescriptor,
            SetFileSecurityW, SetSecurityDescriptorDacl, ACL, ACL_REVISION,
            DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_DESCRIPTOR,
        },
        Storage::FileSystem::FILE_ALL_ACCESS,
    };

    let sid_storage = file_owner_sid(path);
    let sid = sid_storage.as_ptr().cast_mut().cast();
    let sid_size = unsafe {
        // SAFETY: `sid` points to the aligned, valid owner SID copied by
        // `file_owner_sid` and remains live for this synchronous call.
        usize::try_from(GetLengthSid(sid)).expect("owner SID length must fit usize")
    };

    let acl_size_bytes = size_of::<ACL>() + size_of::<u32>() * 2 + sid_size;
    let acl_size =
        u32::try_from(acl_size_bytes).expect("owner-only ACL fixture must fit in a DWORD");
    let mut acl_storage = vec![0u32; acl_size_bytes.div_ceil(size_of::<u32>())];
    let acl = acl_storage.as_mut_ptr().cast::<ACL>();
    let acl_initialized = unsafe {
        // SAFETY: `acl` points to an aligned, writable buffer of `acl_size`
        // bytes reserved for one access-allowed ACE.
        InitializeAcl(acl, acl_size, ACL_REVISION)
    };
    assert_ne!(
        acl_initialized,
        0,
        "InitializeAcl failed: {:?}",
        std::io::Error::last_os_error()
    );
    let ace_added = unsafe {
        // SAFETY: `acl` and the destination owner's `sid` remain live and
        // initialized for this call; `FILE_ALL_ACCESS` keeps the test process
        // able to replace and clean up the deliberately explicit owner ACE.
        AddAccessAllowedAce(acl, ACL_REVISION, FILE_ALL_ACCESS, sid)
    };
    assert_ne!(
        ace_added,
        0,
        "AddAccessAllowedAce failed: {:?}",
        std::io::Error::last_os_error()
    );

    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let descriptor_initialized = unsafe {
        // SAFETY: `descriptor` is a writable security-descriptor value and the
        // revision is the documented SECURITY_DESCRIPTOR_REVISION value.
        InitializeSecurityDescriptor((&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(), 1)
    };
    assert_ne!(
        descriptor_initialized,
        0,
        "InitializeSecurityDescriptor failed: {:?}",
        std::io::Error::last_os_error()
    );
    let dacl_set = unsafe {
        // SAFETY: `descriptor` and `acl` remain live for this synchronous call;
        // the explicit DACL is present and not defaulted.
        SetSecurityDescriptorDacl(
            (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
            1,
            acl,
            0,
        )
    };
    assert_ne!(
        dacl_set,
        0,
        "SetSecurityDescriptorDacl failed: {:?}",
        std::io::Error::last_os_error()
    );

    let encoded = windows_test_path(path);
    let security_set = unsafe {
        // SAFETY: `encoded`, `descriptor`, and its referenced ACL/SID buffers
        // remain live for this synchronous call, and only the DACL is changed.
        SetFileSecurityW(
            encoded.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
        )
    };
    assert_ne!(
        security_set,
        0,
        "SetFileSecurityW failed: {:?}",
        std::io::Error::last_os_error()
    );

    file_dacl(path)
}

#[cfg(windows)]
#[test]
fn direct_artifact_writers_preserve_nondefault_destination_dacl() {
    let cleanup_path = {
        let directory = OwnedArtifactDirectory::new("artifact_windows_acl");
        let baseline = directory.path().join("baseline.txt");
        let rust_output = directory.path().join("bindings.rs");
        let text_output = directory.path().join("report.json");
        std::fs::write(&baseline, "default").unwrap();
        std::fs::write(&rust_output, "old Rust artifact").unwrap();
        std::fs::write(&text_output, "old text artifact").unwrap();

        let default_dacl = file_dacl(&baseline);
        let rust_dacl = set_nondefault_dacl(&rust_output);
        let text_dacl = set_nondefault_dacl(&text_output);
        assert_eq!(rust_dacl, text_dacl, "ACL fixture must be deterministic");
        assert_eq!(
            rust_dacl.aces.len(),
            1,
            "ACL fixture must contain only the explicit owner allow ACE"
        );
        assert!(
            rust_dacl.protected,
            "ACL fixture must protect its explicit DACL from inheritance"
        );
        assert_ne!(
            rust_dacl, default_dacl,
            "ACL fixture must differ from the inherited default DACL"
        );

        let rust_artifact = RustArtifact::from_tokens(quote!(
            pub fn generated() {}
        ))
        .unwrap();
        let text_artifact = TextArtifact::new("generated report");

        rust_artifact.write(&rust_output).unwrap();
        text_artifact.write(&text_output).unwrap();

        assert_eq!(
            std::fs::read_to_string(&rust_output).unwrap(),
            rust_artifact.contents()
        );
        assert_eq!(
            std::fs::read_to_string(&text_output).unwrap(),
            text_artifact.contents()
        );
        assert_eq!(file_dacl(&rust_output), rust_dacl);
        assert_eq!(file_dacl(&text_output), text_dacl);
        directory.path().to_owned()
    };
    assert!(
        !cleanup_path.exists(),
        "exclusive artifact fixture must be removed by its owner"
    );
}

#[cfg(windows)]
struct OwnedUncArtifactDirectory {
    path: PathBuf,
}

#[cfg(windows)]
impl OwnedUncArtifactDirectory {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(windows)]
impl Drop for OwnedUncArtifactDirectory {
    fn drop(&mut self) {
        // `create_dir` below establishes exclusive ownership of this child; never remove the
        // caller-provided UNC root or a pre-existing child.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(windows)]
fn configured_unc_artifact_directory() -> OwnedUncArtifactDirectory {
    let root = PathBuf::from(
        std::env::var_os("PYO3_BINDGEN_TEST_UNC_ROOT")
            .expect("PYO3_BINDGEN_TEST_UNC_ROOT must name a writable UNC share"),
    );
    let is_unc = matches!(
        root.components().next(),
        Some(std::path::Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                std::path::Prefix::UNC(..) | std::path::Prefix::VerbatimUNC(..)
            )
    );
    assert!(
        is_unc,
        "PYO3_BINDGEN_TEST_UNC_ROOT must be a UNC path when configured"
    );
    let root_metadata = std::fs::metadata(&root)
        .expect("PYO3_BINDGEN_TEST_UNC_ROOT must name an existing UNC directory");
    assert!(
        root_metadata.is_dir(),
        "PYO3_BINDGEN_TEST_UNC_ROOT must name an existing UNC directory"
    );

    let directory = root.join(format!(
        "pyo3_bindgen_engine_artifact_unc_{}_{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed),
    ));
    std::fs::create_dir(&directory).expect(
        "configured UNC child must be creatable exclusively; a stale collision is not removed",
    );
    OwnedUncArtifactDirectory { path: directory }
}

#[cfg(windows)]
#[test]
#[ignore = "requires a writable PYO3_BINDGEN_TEST_UNC_ROOT UNC share"]
fn direct_artifact_writers_publish_to_a_configured_unc_path() {
    // A disposable UNC share must be supplied through this explicit opt-in hook.
    let directory = configured_unc_artifact_directory();
    let rust_output = directory.path().join("bindings.rs");
    let text_output = directory.path().join("report.json");
    let rust_artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();
    let text_artifact = TextArtifact::new("generated report");

    rust_artifact.write(&rust_output).unwrap();
    text_artifact.write(&text_output).unwrap();

    assert_eq!(
        std::fs::read_to_string(rust_output).unwrap(),
        rust_artifact.contents()
    );
    assert_eq!(
        std::fs::read_to_string(text_output).unwrap(),
        text_artifact.contents()
    );
}

#[test]
fn text_check_rejects_non_utf8_existing_content() {
    let directory = temp_dir("artifact_non_utf8");
    let output = directory.join("bindings.rs");
    std::fs::write(&output, [0xff]).unwrap();
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();

    let error = artifact.check(&output).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn text_check_rejects_a_directory_without_reading_it() {
    let directory = temp_dir("artifact_check_directory");
    let output = directory.join("bindings.rs");
    std::fs::create_dir(&output).unwrap();
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();

    let error = artifact.check(&output).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("not a regular file"));
    assert!(output.is_dir());
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn text_check_rejects_a_symlink_without_following_it() {
    use std::os::unix::fs::symlink;

    let directory = temp_dir("artifact_check_symlink");
    let target = directory.join("target.rs");
    let output = directory.join("bindings.rs");
    std::fs::write(&target, "pub fn generated() {}\n").unwrap();
    symlink(&target, &output).unwrap();
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();

    let error = artifact.check(&output).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("symbolic link"));
    assert!(std::fs::symlink_metadata(&output)
        .unwrap()
        .file_type()
        .is_symlink());
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn text_check_rejects_a_unix_socket_without_opening_it() {
    use std::os::unix::{fs::FileTypeExt, net::UnixListener};

    let directory = temp_dir("artifact_check_socket");
    let output = directory.join("bindings.sock");
    let listener = UnixListener::bind(&output).unwrap();
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();

    let error = artifact.check(&output).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("not a regular file"));
    assert!(std::fs::symlink_metadata(&output)
        .unwrap()
        .file_type()
        .is_socket());
    drop(listener);
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn codegen_build_writes_the_canonical_formatted_artifact() {
    let expected = RustArtifact::from_tokens(codegen().generate().unwrap()).unwrap();
    let directory = temp_dir("codegen_build");
    let output = directory.join("nested/bindings.rs");

    codegen().build(&output).unwrap();

    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        expected.contents()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn codegen_build_report_describes_the_exact_canonical_file_bytes() {
    let directory = temp_dir("codegen_build_report");
    let output = directory.join("nested/bindings.rs");

    let report = codegen().build_with_report(&output).unwrap();
    let bytes = std::fs::read(&output).unwrap();

    assert_eq!(
        report.outputs,
        vec![OutputMetadata::from_path_bytes(
            "file",
            Some(&output),
            &bytes,
        )]
    );
    assert_eq!(report.outputs[0].hash, stable_hash(&bytes));
    assert_eq!(report.outputs[0].bytes, bytes.len());
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn output_metadata_documents_non_utf8_paths_as_display_only() {
    use std::os::unix::ffi::OsStringExt;

    let path = PathBuf::from(std::ffi::OsString::from_vec(b"bindings-\xff.rs".to_vec()));
    let bytes = b"generated bindings";

    let output = OutputMetadata::from_path_bytes("file", Some(&path), bytes);
    let display_path = path.display().to_string();

    assert_eq!(output.path.as_deref(), Some(display_path.as_str()));
    assert_eq!(output.hash, stable_hash(bytes));
    assert_eq!(output.bytes, bytes.len());
}

#[test]
fn codegen_generation_report_describes_unformatted_token_text() {
    let generated = codegen().generate_with_report().unwrap();
    let token_text = generated.tokens.to_string();
    let canonical = RustArtifact::from_tokens(generated.tokens.clone()).unwrap();

    assert_ne!(token_text.as_bytes(), canonical.bytes());
    assert_eq!(
        generated.report.outputs,
        vec![OutputMetadata::from_bytes(
            "token_stream",
            None,
            token_text.as_bytes(),
        )]
    );
}

#[test]
fn incremental_and_batched_external_ir_have_identical_canonical_artifacts() {
    let modules = (0..100)
        .map(|index| ModuleIr {
            name: format!("module_{index:03}"),
            is_package: false,
            docstring: None,
            classes: vec![],
            enums: vec![],
            functions: vec![],
            properties: vec![],
            submodules: vec![],
            prelude: vec![],
            imports: vec![],
            type_vars: vec![],
        })
        .collect::<Vec<_>>();
    let config = Config::builder()
        .generate_provenance(false)
        .generate_preludes(false)
        .build();

    let mut incremental = Codegen::new(config.clone());
    for module in modules.iter().cloned() {
        incremental = incremental
            .module_from_ir(module)
            .expect("incremental raw schema registration must remain infallible");
    }
    let batch = Codegen::new(config)
        .modules_from_ir(SchemaIr { modules })
        .expect("batch raw schema registration must remain infallible");

    let incremental = RustArtifact::from_tokens(
        incremental
            .generate()
            .expect("incremental external IR must generate"),
    )
    .unwrap();
    let batch =
        RustArtifact::from_tokens(batch.generate().expect("batched external IR must generate"))
            .unwrap();

    assert_eq!(incremental.bytes(), batch.bytes());
    assert_eq!(incremental.hash(), batch.hash());
}

#[cfg(unix)]
#[test]
fn rust_artifact_refuses_a_symlink_destination() {
    use std::os::unix::fs::symlink;

    let directory = temp_dir("artifact_symlink");
    let target = directory.join("target.rs");
    let output = directory.join("bindings.rs");
    std::fs::write(&target, "must stay unchanged").unwrap();
    symlink(&target, &output).unwrap();

    let artifact = RustArtifact::from_tokens(quote!(
        pub fn replacement() {}
    ))
    .unwrap();
    let error = artifact.write(&output).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "must stay unchanged"
    );
    assert!(std::fs::symlink_metadata(&output)
        .unwrap()
        .file_type()
        .is_symlink());
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn rust_artifact_refuses_a_directory_destination() {
    let directory = temp_dir("artifact_directory_destination");
    let output = directory.join("bindings.rs");
    std::fs::create_dir(&output).unwrap();
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn replacement() {}
    ))
    .unwrap();

    let error = artifact.write(&output).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("not a regular file"));
    assert!(output.is_dir(), "the destination directory must survive");
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn rust_artifact_refuses_a_unix_socket_destination() {
    use std::os::unix::{fs::FileTypeExt, net::UnixListener};

    let directory = temp_dir("artifact_socket_destination");
    let output = directory.join("bindings.sock");
    let listener = UnixListener::bind(&output).unwrap();
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn replacement() {}
    ))
    .unwrap();

    let error = artifact.write(&output).unwrap_err();

    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert!(error.to_string().contains("not a regular file"));
    assert!(
        std::fs::symlink_metadata(&output)
            .unwrap()
            .file_type()
            .is_socket(),
        "the destination socket must not be replaced"
    );
    drop(listener);
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn rust_artifact_preserves_special_destination_mode_bits() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir("artifact_special_mode");
    let output = directory.join("bindings.rs");
    std::fs::write(&output, "old").unwrap();
    std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o4750)).unwrap();
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn replacement() {}
    ))
    .unwrap();

    artifact.write(&output).unwrap();

    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o7777,
        0o4750
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn rust_artifact_write_returns_a_durability_error_after_post_rename_failure() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir("artifact_parent_sync_failure");
    let output = directory.join("bindings.rs");
    std::fs::write(&output, "old").unwrap();

    let original_mode = std::fs::metadata(&directory).unwrap().permissions().mode();
    // Keep write+execute (needed to create the staged file and rename it into
    // place) but drop read, so the post-rename `File::open(parent)` used to
    // fsync the directory entry fails with `PermissionDenied` even though the
    // rename itself has already durably completed.
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o300)).unwrap();

    // Some environments (e.g. running as root) don't enforce this restriction;
    // skip rather than produce a false pass in that case.
    if std::fs::File::open(&directory).is_ok() {
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(original_mode))
            .unwrap();
        let _ = std::fs::remove_dir_all(directory);
        return;
    }

    let artifact = RustArtifact::from_tokens(quote!(
        pub fn replacement() {}
    ))
    .unwrap();

    let result = artifact.write(&output);

    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(original_mode)).unwrap();
    let error = result.unwrap_err();
    assert!(error.to_string().contains("replacement already occurred"));
    assert_eq!(
        std::fs::read_to_string(&output).unwrap(),
        artifact.contents()
    );
    let _ = std::fs::remove_dir_all(directory);
}

#[cfg(unix)]
#[test]
fn rust_artifact_publishes_new_files_with_private_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let directory = temp_dir("artifact_private_mode");
    let output = directory.join("bindings.rs");
    let artifact = RustArtifact::from_tokens(quote!(
        pub fn generated() {}
    ))
    .unwrap();

    artifact.write(&output).unwrap();

    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let _ = std::fs::remove_dir_all(directory);
}
