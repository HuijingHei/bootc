//! # Implementation of container build lints
//!
//! This module implements `bootc container lint`.

// Unfortunately needed here to work with linkme
#![allow(unsafe_code)]

use std::collections::BTreeSet;
use std::env::consts::ARCH;
use std::os::unix::ffi::OsStrExt;

use anyhow::Result;
use camino::{Utf8Path, Utf8PathBuf};
use cap_std::fs::Dir;
use cap_std_ext::cap_std;
use cap_std_ext::cap_std::fs::MetadataExt;
use cap_std_ext::dirext::CapStdExtDirExt as _;
use fn_error_context::context;
use indoc::indoc;
use linkme::distributed_slice;
use ostree_ext::ostree_prepareroot;
use serde::Serialize;

/// Reference to embedded default baseimage content that should exist.
const BASEIMAGE_REF: &str = "usr/share/doc/bootc/baseimage/base";

/// A lint check has failed.
#[derive(thiserror::Error, Debug)]
struct LintError(String);

/// The outer error is for unexpected fatal runtime problems; the
/// inner error is for the lint failing in an expected way.
type LintResult = Result<std::result::Result<(), LintError>>;

/// Everything is OK - we didn't encounter a runtime error, and
/// the targeted check passed.
fn lint_ok() -> LintResult {
    Ok(Ok(()))
}

/// We successfully found a lint failure.
fn lint_err(msg: impl AsRef<str>) -> LintResult {
    Ok(Err(LintError::new(msg)))
}

impl std::fmt::Display for LintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl LintError {
    fn new(msg: impl AsRef<str>) -> Self {
        Self(msg.as_ref().to_owned())
    }
}

type LintFn = fn(&Dir) -> LintResult;
#[distributed_slice]
pub(crate) static LINTS: [Lint];

/// The classification of a lint type.
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
enum LintType {
    /// If this fails, it is known to be fatal - the system will not install or
    /// is effectively guaranteed to fail at runtime.
    Fatal,
    /// This is not a fatal problem, but something you likely want to fix.
    Warning,
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum WarningDisposition {
    AllowWarnings,
    FatalWarnings,
}

#[derive(Debug, Copy, Clone, Serialize, PartialEq, Eq)]
pub(crate) enum RootType {
    Running,
    Alternative,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
struct Lint {
    name: &'static str,
    #[serde(rename = "type")]
    ty: LintType,
    #[serde(skip)]
    f: LintFn,
    description: &'static str,
    // Set if this only applies to a specific root type.
    #[serde(skip_serializing_if = "Option::is_none")]
    root_type: Option<RootType>,
}

impl Lint {
    pub(crate) const fn new_fatal(
        name: &'static str,
        description: &'static str,
        f: LintFn,
    ) -> Self {
        Lint {
            name: name,
            ty: LintType::Fatal,
            f: f,
            description: description,
            root_type: None,
        }
    }

    pub(crate) const fn new_warning(
        name: &'static str,
        description: &'static str,
        f: LintFn,
    ) -> Self {
        Lint {
            name: name,
            ty: LintType::Warning,
            f: f,
            description: description,
            root_type: None,
        }
    }
}

pub(crate) fn lint_list(output: impl std::io::Write) -> Result<()> {
    // Dump in yaml format by default, it's readable enough
    serde_yaml::to_writer(output, &*LINTS)?;
    Ok(())
}

/// check for the existence of the /var/run directory
/// if it exists we need to check that it links to /run if not error
/// if it does not exist error.
#[context("Linting")]
pub(crate) fn lint(
    root: &Dir,
    warning_disposition: WarningDisposition,
    root_type: RootType,
    mut output: impl std::io::Write,
) -> Result<()> {
    let mut fatal = 0usize;
    let mut warnings = 0usize;
    let mut passed = 0usize;
    let mut skipped = 0usize;
    for lint in LINTS {
        let name = lint.name;

        if let Some(lint_root_type) = lint.root_type {
            if lint_root_type != root_type {
                skipped += 1;
            }
        }

        let r = match (lint.f)(&root) {
            Ok(r) => r,
            Err(e) => anyhow::bail!("Unexpected runtime error running lint {name}: {e}"),
        };

        if let Err(e) = r {
            match lint.ty {
                LintType::Fatal => {
                    writeln!(output, "Failed lint: {name}: {e}")?;
                    fatal += 1;
                }
                LintType::Warning => {
                    writeln!(output, "Lint warning: {name}: {e}")?;
                    warnings += 1;
                }
            }
        } else {
            // We'll be quiet for now
            tracing::debug!("OK {name} (type={:?})", lint.ty);
            passed += 1;
        }
    }
    writeln!(output, "Checks passed: {passed}")?;
    if skipped > 0 {
        writeln!(output, "Checks skipped: {skipped}")?;
    }
    let fatal = if matches!(warning_disposition, WarningDisposition::FatalWarnings) {
        fatal + warnings
    } else {
        fatal
    };
    if warnings > 0 {
        writeln!(output, "Warnings: {warnings}")?;
    }
    if fatal > 0 {
        anyhow::bail!("Checks failed: {fatal}")
    }
    Ok(())
}

#[distributed_slice(LINTS)]
static LINT_VAR_RUN: Lint = Lint::new_fatal(
    "var-run",
    "Check for /var/run being a physical directory; this is always a bug.",
    check_var_run,
);
fn check_var_run(root: &Dir) -> LintResult {
    if let Some(meta) = root.symlink_metadata_optional("var/run")? {
        if !meta.is_symlink() {
            return lint_err("Not a symlink: var/run");
        }
    }
    lint_ok()
}

#[distributed_slice(LINTS)]
static LINT_BUILDAH_INJECTED: Lint = Lint {
    name: "buildah-injected",
    description: indoc::indoc! { "
        Check for an invalid /etc/hostname or /etc/resolv.conf that may have been injected by
        a container build system." },
    ty: LintType::Warning,
    f: check_buildah_injected,
    // This one doesn't make sense to run looking at the running root,
    // because we do expect /etc/hostname to be injected as
    root_type: Some(RootType::Alternative),
};
fn check_buildah_injected(root: &Dir) -> LintResult {
    const RUNTIME_INJECTED: &[&str] = &["etc/hostname", "etc/resolv.conf"];
    for ent in RUNTIME_INJECTED {
        if let Some(meta) = root.symlink_metadata_optional(ent)? {
            if meta.is_file() && meta.size() == 0 {
                return lint_err(format!("/{ent} is an empty file; this may have been synthesized by a container runtime."));
            }
        }
    }
    lint_ok()
}

#[distributed_slice(LINTS)]
static LINT_ETC_USRUSETC: Lint = Lint::new_fatal(
    "etc-usretc",
    indoc! { r#"
Verify that only one of /etc or /usr/etc exist. You should only have /etc
in a container image. It will cause undefined behavior to have both /etc
and /usr/etc.
"# },
    check_usretc,
);
fn check_usretc(root: &Dir) -> LintResult {
    let etc_exists = root.symlink_metadata_optional("etc")?.is_some();
    // For compatibility/conservatism don't bomb out if there's no /etc.
    if !etc_exists {
        return lint_ok();
    }
    // But having both /etc and /usr/etc is not something we want to support.
    if root.symlink_metadata_optional("usr/etc")?.is_some() {
        return lint_err(
            "Found /usr/etc - this is a bootc implementation detail and not supported to use in containers"
        );
    }
    lint_ok()
}

/// Validate that we can parse the /usr/lib/bootc/kargs.d files.
#[distributed_slice(LINTS)]
static LINT_KARGS: Lint = Lint::new_fatal(
    "bootc-kargs",
    "Verify syntax of /usr/lib/bootc/kargs.d.",
    check_parse_kargs,
);
fn check_parse_kargs(root: &Dir) -> LintResult {
    let args = crate::kargs::get_kargs_in_root(root, ARCH)?;
    tracing::debug!("found kargs: {args:?}");
    lint_ok()
}

#[distributed_slice(LINTS)]
static LINT_KERNEL: Lint = Lint::new_fatal(
    "kernel",
    indoc! { r#"
             Check for multiple kernels, i.e. multiple directories of the form /usr/lib/modules/$kver.
             Only one kernel is supported in an image.
     "# },
    check_kernel,
);
fn check_kernel(root: &Dir) -> LintResult {
    let result = ostree_ext::bootabletree::find_kernel_dir_fs(&root)?;
    tracing::debug!("Found kernel: {:?}", result);
    lint_ok()
}

// This one can be lifted in the future, see https://github.com/containers/bootc/issues/975
#[distributed_slice(LINTS)]
static LINT_UTF8: Lint = Lint::new_fatal(
    "utf8",
    indoc! { r#"
Check for non-UTF8 filenames. Currently, the ostree backend of bootc only supports
UTF-8 filenames. Non-UTF8 filenames will cause a fatal error.
"#},
    check_utf8,
);
fn check_utf8(dir: &Dir) -> LintResult {
    for entry in dir.entries()? {
        let entry = entry?;
        let name = entry.file_name();

        let Some(strname) = &name.to_str() else {
            // will escape nicely like "abc\xFFdéf"
            return lint_err(format!("/: Found non-utf8 filename {name:?}"));
        };

        let ifmt = entry.file_type()?;
        if ifmt.is_symlink() {
            let target = dir.read_link_contents(&name)?;
            if !target.to_str().is_some() {
                return lint_err(format!("/{strname}: Found non-utf8 symlink target"));
            }
        } else if ifmt.is_dir() {
            let Some(subdir) = crate::utils::open_dir_noxdev(dir, entry.file_name())? else {
                continue;
            };
            if let Err(err) = check_utf8(&subdir)? {
                // Try to do the path pasting only in the event of an error
                return lint_err(format!("/{strname}{err}"));
            }
        }
    }
    lint_ok()
}

/// Check for a few files and directories we expect in the base image.
fn check_baseimage_root_norecurse(dir: &Dir) -> LintResult {
    // Check /sysroot
    let meta = dir.symlink_metadata_optional("sysroot")?;
    match meta {
        Some(meta) if !meta.is_dir() => return lint_err("Expected a directory for /sysroot"),
        None => return lint_err("Missing /sysroot"),
        _ => {}
    }

    // Check /ostree -> sysroot/ostree
    let Some(meta) = dir.symlink_metadata_optional("ostree")? else {
        return lint_err("Missing ostree -> sysroot/ostree link");
    };
    if !meta.is_symlink() {
        return lint_err("/ostree should be a symlink");
    }
    let link = dir.read_link_contents("ostree")?;
    let expected = "sysroot/ostree";
    if link.as_os_str().as_bytes() != expected.as_bytes() {
        return lint_err("Expected /ostree -> {expected}, not {link:?}");
    }

    let config = ostree_prepareroot::require_config_from_root(dir)?;
    if !ostree_prepareroot::overlayfs_enabled_in_config(&config)? {
        return lint_err("{prepareroot_path} does not have composefs enabled");
    }

    lint_ok()
}

/// Check ostree-related base image content.
#[distributed_slice(LINTS)]
static LINT_BASEIMAGE_ROOT: Lint = Lint::new_fatal(
    "baseimage-root",
    indoc! { r#"
Check that expected files are present in the root of the filesystem; such
as /sysroot and a composefs configuration for ostree. More in
<https://containers.github.io/bootc/bootc-images.html#standard-image-content>.
"#},
    check_baseimage_root,
);
fn check_baseimage_root(dir: &Dir) -> LintResult {
    if let Err(e) = check_baseimage_root_norecurse(dir)? {
        return Ok(Err(e));
    }
    // If we have our own documentation with the expected root contents
    // embedded, then check that too! Mostly just because recursion is fun.
    if let Some(dir) = dir.open_dir_optional(BASEIMAGE_REF)? {
        if let Err(e) = check_baseimage_root_norecurse(&dir)? {
            return Ok(Err(e));
        }
    }
    lint_ok()
}

fn collect_nonempty_regfiles(
    root: &Dir,
    path: &Utf8Path,
    out: &mut BTreeSet<Utf8PathBuf>,
) -> Result<()> {
    for entry in root.entries_utf8()? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let path = path.join(entry.file_name()?);
        if ty.is_file() {
            let meta = entry.metadata()?;
            if meta.size() > 0 {
                out.insert(path);
            }
        } else if ty.is_dir() {
            let d = entry.open_dir()?;
            collect_nonempty_regfiles(d.as_cap_std(), &path, out)?;
        }
    }
    Ok(())
}

#[distributed_slice(LINTS)]
static LINT_VARLOG: Lint = Lint::new_warning(
    "var-log",
    indoc! { r#"
Check for non-empty regular files in `/var/log`. It is often undesired
to ship log files in container images. Log files in general are usually
per-machine state in `/var`. Additionally, log files often include
timestamps, causing unreproducible container images, and may contain
sensitive build system information.
"#},
    check_varlog,
);
fn check_varlog(root: &Dir) -> LintResult {
    let Some(d) = root.open_dir_optional("var/log")? else {
        return lint_ok();
    };
    let mut nonempty_regfiles = BTreeSet::new();
    collect_nonempty_regfiles(&d, "/var/log".into(), &mut nonempty_regfiles)?;
    let mut nonempty_regfiles = nonempty_regfiles.into_iter();
    let Some(first) = nonempty_regfiles.next() else {
        return lint_ok();
    };
    let others = nonempty_regfiles.len();
    let others = if others > 0 {
        format!(" (and {others} more)")
    } else {
        "".into()
    };
    lint_err(format!("Found non-empty logfile: {first}{others}"))
}

#[distributed_slice(LINTS)]
static LINT_NONEMPTY_BOOT: Lint = Lint::new_warning(
    "nonempty-boot",
    indoc! { r#"
The `/boot` directory should be present, but empty. The kernel
content should be in /usr/lib/modules instead in the container image.
Any content here in the container image will be masked at runtime.
"#},
    check_boot,
);
fn check_boot(root: &Dir) -> LintResult {
    let Some(d) = root.open_dir_optional("boot")? else {
        return lint_err(format!("Missing /boot directory"));
    };
    let mut entries = d.entries()?;
    let Some(ent) = entries.next() else {
        return lint_ok();
    };
    let ent = ent?;
    let first = ent.file_name();
    let others = entries.count();
    let others = if others > 0 {
        format!(" (and {others} more)")
    } else {
        "".into()
    };
    lint_err(format!("Found non-empty /boot: {first:?}{others}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Result<cap_std_ext::cap_tempfile::TempDir> {
        let tempdir = cap_std_ext::cap_tempfile::tempdir(cap_std::ambient_authority())?;
        Ok(tempdir)
    }

    fn passing_fixture() -> Result<cap_std_ext::cap_tempfile::TempDir> {
        let root = cap_std_ext::cap_tempfile::tempdir(cap_std::ambient_authority())?;
        root.create_dir_all("usr/lib/modules/5.7.2")?;
        root.write("usr/lib/modules/5.7.2/vmlinuz", "vmlinuz")?;

        root.create_dir("boot")?;
        root.create_dir("sysroot")?;
        root.symlink_contents("sysroot/ostree", "ostree")?;

        const PREPAREROOT_PATH: &str = "usr/lib/ostree/prepare-root.conf";
        const PREPAREROOT: &str =
            include_str!("../../baseimage/base/usr/lib/ostree/prepare-root.conf");
        root.create_dir_all(Utf8Path::new(PREPAREROOT_PATH).parent().unwrap())?;
        root.atomic_write(PREPAREROOT_PATH, PREPAREROOT)?;

        Ok(root)
    }

    #[test]
    fn test_var_run() -> Result<()> {
        let root = &fixture()?;
        // This one should pass
        check_var_run(root).unwrap().unwrap();
        root.create_dir_all("var/run/foo")?;
        assert!(check_var_run(root).unwrap().is_err());
        root.remove_dir_all("var/run")?;
        // Now we should pass again
        check_var_run(root).unwrap().unwrap();
        Ok(())
    }

    #[test]
    fn test_lint_main() -> Result<()> {
        let root = &passing_fixture()?;
        let mut out = Vec::new();
        let warnings = WarningDisposition::FatalWarnings;
        let root_type = RootType::Running;
        lint(root, warnings, root_type, &mut out).unwrap();
        root.create_dir_all("var/run/foo")?;
        let mut out = Vec::new();
        assert!(lint(root, warnings, root_type, &mut out).is_err());
        Ok(())
    }

    #[test]
    fn test_kernel_lint() -> Result<()> {
        let root = &fixture()?;
        // This one should pass
        check_kernel(root).unwrap().unwrap();
        root.create_dir_all("usr/lib/modules/5.7.2")?;
        root.write("usr/lib/modules/5.7.2/vmlinuz", "old vmlinuz")?;
        root.create_dir_all("usr/lib/modules/6.3.1")?;
        root.write("usr/lib/modules/6.3.1/vmlinuz", "new vmlinuz")?;
        assert!(check_kernel(root).is_err());
        root.remove_dir_all("usr/lib/modules/5.7.2")?;
        // Now we should pass again
        check_kernel(root).unwrap().unwrap();
        Ok(())
    }

    #[test]
    fn test_kargs() -> Result<()> {
        let root = &fixture()?;
        check_parse_kargs(root).unwrap().unwrap();
        root.create_dir_all("usr/lib/bootc")?;
        root.write("usr/lib/bootc/kargs.d", "not a directory")?;
        assert!(check_parse_kargs(root).is_err());
        Ok(())
    }

    #[test]
    fn test_usr_etc() -> Result<()> {
        let root = &fixture()?;
        // This one should pass
        check_usretc(root).unwrap().unwrap();
        root.create_dir_all("etc")?;
        root.create_dir_all("usr/etc")?;
        assert!(check_usretc(root).unwrap().is_err());
        root.remove_dir_all("etc")?;
        // Now we should pass again
        check_usretc(root).unwrap().unwrap();
        Ok(())
    }

    #[test]
    fn test_varlog() -> Result<()> {
        let root = &fixture()?;
        check_varlog(root).unwrap().unwrap();
        root.create_dir_all("var/log")?;
        check_varlog(root).unwrap().unwrap();
        root.symlink_contents("../../usr/share/doc/systemd/README.logs", "var/log/README")?;
        check_varlog(root).unwrap().unwrap();

        root.atomic_write("var/log/somefile.log", "log contents")?;
        let Err(e) = check_varlog(root).unwrap() else {
            unreachable!()
        };
        assert_eq!(
            e.to_string(),
            "Found non-empty logfile: /var/log/somefile.log"
        );

        root.create_dir_all("var/log/someproject")?;
        root.atomic_write("var/log/someproject/audit.log", "audit log")?;
        root.atomic_write("var/log/someproject/info.log", "info")?;
        let Err(e) = check_varlog(root).unwrap() else {
            unreachable!()
        };
        assert_eq!(
            e.to_string(),
            "Found non-empty logfile: /var/log/somefile.log (and 2 more)"
        );

        Ok(())
    }

    #[test]
    fn test_boot() -> Result<()> {
        let root = &passing_fixture()?;
        check_boot(&root).unwrap().unwrap();
        root.create_dir("boot/somesubdir")?;
        let Err(e) = check_boot(&root).unwrap() else {
            unreachable!()
        };
        assert!(e.to_string().contains("somesubdir"));

        Ok(())
    }

    #[test]
    fn test_non_utf8() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

        let root = &fixture().unwrap();

        // Try to create some adversarial symlink situations to ensure the walk doesn't crash
        root.create_dir("subdir").unwrap();
        // Self-referential symlinks
        root.symlink("self", "self").unwrap();
        // Infinitely looping dir symlinks
        root.symlink("..", "subdir/parent").unwrap();
        // Broken symlinks
        root.symlink("does-not-exist", "broken").unwrap();
        // Out-of-scope symlinks
        root.symlink("../../x", "escape").unwrap();
        // Should be fine
        check_utf8(root).unwrap().unwrap();

        // But this will cause an issue
        let baddir = OsStr::from_bytes(b"subdir/2/bad\xffdir");
        root.create_dir("subdir/2").unwrap();
        root.create_dir(baddir).unwrap();
        let Err(err) = check_utf8(root).unwrap() else {
            unreachable!("Didn't fail");
        };
        assert_eq!(
            err.to_string(),
            r#"/subdir/2/: Found non-utf8 filename "bad\xFFdir""#
        );
        root.remove_dir(baddir).unwrap(); // Get rid of the problem
        check_utf8(root).unwrap().unwrap(); // Check it

        // Create a new problem in the form of a regular file
        let badfile = OsStr::from_bytes(b"regular\xff");
        root.write(badfile, b"Hello, world!\n").unwrap();
        let Err(err) = check_utf8(root).unwrap() else {
            unreachable!("Didn't fail");
        };
        assert_eq!(
            err.to_string(),
            r#"/: Found non-utf8 filename "regular\xFF""#
        );
        root.remove_file(badfile).unwrap(); // Get rid of the problem
        check_utf8(root).unwrap().unwrap(); // Check it

        // And now test invalid symlink targets
        root.symlink(badfile, "subdir/good-name").unwrap();
        let Err(err) = check_utf8(root).unwrap() else {
            unreachable!("Didn't fail");
        };
        assert_eq!(
            err.to_string(),
            r#"/subdir/good-name: Found non-utf8 symlink target"#
        );
        root.remove_file("subdir/good-name").unwrap(); // Get rid of the problem
        check_utf8(root).unwrap().unwrap(); // Check it

        // Finally, test a self-referential symlink with an invalid name.
        // We should spot the invalid name before we check the target.
        root.symlink(badfile, badfile).unwrap();
        let Err(err) = check_utf8(root).unwrap() else {
            unreachable!("Didn't fail");
        };
        assert_eq!(
            err.to_string(),
            r#"/: Found non-utf8 filename "regular\xFF""#
        );
        root.remove_file(badfile).unwrap(); // Get rid of the problem
        check_utf8(root).unwrap().unwrap(); // Check it
    }

    #[test]
    fn test_baseimage_root() -> Result<()> {
        let td = fixture()?;

        // An empty root should fail our test
        assert!(check_baseimage_root(&td).unwrap().is_err());

        drop(td);
        let td = passing_fixture()?;
        check_baseimage_root(&td).unwrap().unwrap();
        Ok(())
    }

    #[test]
    fn test_buildah_injected() -> Result<()> {
        let td = fixture()?;
        td.create_dir("etc")?;
        assert!(check_buildah_injected(&td).unwrap().is_ok());
        td.write("etc/hostname", b"")?;
        assert!(check_buildah_injected(&td).unwrap().is_err());
        td.write("etc/hostname", b"some static hostname")?;
        assert!(check_buildah_injected(&td).unwrap().is_ok());
        Ok(())
    }

    #[test]
    fn test_list() {
        let mut r = Vec::new();
        lint_list(&mut r).unwrap();
        let lints: Vec<serde_yaml::Value> = serde_yaml::from_slice(&r).unwrap();
        assert_eq!(lints.len(), LINTS.len());
    }
}
