//! Host-integration tests for the authorized_keys writer against fixture home directories.
//! Foreign lines must survive byte for byte; unsafe targets must be refused.

use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::sync::Arc;

use nix::unistd::{getgid, getuid};
use yanto_core::authkeys::{self, BEGIN_MARKER, END_MARKER};
use yanto_core::ports::{AuthorizedKeysPort, KeysEdit, UnixUser};
use yanto_core::testing::users::FakeHostUsers;
use yanto_core::{CoreError, ErrorKind};
use yanto_host::HostAuthorizedKeys;

struct Fx {
    _dir: tempfile::TempDir,
    home: PathBuf,
    port: HostAuthorizedKeys,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home/tester");
    std::fs::create_dir_all(&home).unwrap();
    let users = Arc::new(FakeHostUsers::new(&dir.path().join("home")));
    users.add(UnixUser {
        name: "tester".into(),
        uid: getuid().as_raw(),
        gid: getgid().as_raw(),
        home: home.clone(),
        shell: "/bin/sh".into(),
    });
    users.add(UnixUser {
        name: "relhome".into(),
        uid: 5,
        gid: 5,
        home: "relative/home".into(),
        shell: "/bin/sh".into(),
    });
    Fx {
        port: HostAuthorizedKeys::new(users),
        _dir: dir,
        home,
    }
}

impl Fx {
    fn ak(&self) -> PathBuf {
        self.home.join(".ssh/authorized_keys")
    }
}

fn key(seed: u8, comment: &str) -> String {
    use base64::Engine as _;
    let mut blob = Vec::new();
    for part in [&b"ssh-ed25519"[..], &[seed; 32][..]] {
        blob.extend_from_slice(&(part.len() as u32).to_be_bytes());
        blob.extend_from_slice(part);
    }
    format!(
        "ssh-ed25519 {} {comment}",
        base64::engine::general_purpose::STANDARD.encode(blob)
    )
}

/// An edit that sets the Yanto block to `lines`.
fn set_block(lines: Vec<String>) -> impl FnMut(&[u8]) -> Result<Option<KeysEdit>, CoreError> {
    move |bytes| {
        let s = authkeys::splice(bytes, &lines).map_err(|e| CoreError::conflict(e.to_string()))?;
        Ok(Some(KeysEdit {
            bytes: s.bytes,
            expected_outside: s.expected_outside,
        }))
    }
}

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().mode() & 0o7777
}

#[test]
fn creates_ssh_dir_and_file_with_strict_modes() {
    let fx = fixture();
    let k = key(1, "yanto:akey_1:laptop");
    let r = fx
        .port
        .update("tester", &mut set_block(vec![k.clone()]))
        .unwrap();
    assert!(r.changed && r.backup_path.is_none());
    assert_eq!(mode(&fx.home.join(".ssh")), 0o700);
    assert_eq!(mode(&fx.ak()), 0o600);
    assert_eq!(std::fs::metadata(fx.ak()).unwrap().uid(), getuid().as_raw());
    let text = std::fs::read_to_string(fx.ak()).unwrap();
    assert_eq!(text, format!("{BEGIN_MARKER}\n{k}\n{END_MARKER}\n"));
    let leftovers: Vec<_> = std::fs::read_dir(fx.home.join(".ssh"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        leftovers,
        ["authorized_keys"],
        "no temp or bak files on first write"
    );
}

/// The fixture files from the acceptance criteria: CRLF lines, no trailing newline, options, unicode.
fn foreign_fixtures() -> Vec<Vec<u8>> {
    vec![
        format!("{}\n# my keys\n", key(9, "manual")).into_bytes(),
        format!("{}\r\n{}\r\n# crlf\r\n", key(8, "a"), key(7, "b")).into_bytes(),
        format!("command=\"echo hi\",no-pty {} ci", key(6, "")).into_bytes(),
        "# only a comment, no newline".as_bytes().to_vec(),
        "# caf\u{e9} \u{1f511}\n".as_bytes().to_vec(),
        b"\n\n\n".to_vec(),
        Vec::new(),
    ]
}

#[test]
fn outside_content_is_byte_identical_across_add_replace_and_remove() {
    for foreign in foreign_fixtures() {
        let fx = fixture();
        std::fs::create_dir_all(fx.home.join(".ssh")).unwrap();
        std::fs::set_permissions(fx.home.join(".ssh"), std::fs::Permissions::from_mode(0o700))
            .unwrap();
        std::fs::write(fx.ak(), &foreign).unwrap();
        let expected_outside = {
            let mut v = foreign.clone();
            if !v.is_empty() && !v.ends_with(b"\n") {
                v.push(b'\n');
            }
            v
        };
        for lines in [
            vec![key(1, "y1")],
            vec![key(1, "y1"), key(2, "y2")],
            vec![key(3, "y3")],
            vec![],
        ] {
            fx.port.update("tester", &mut set_block(lines)).unwrap();
            let now = std::fs::read(fx.ak()).unwrap();
            assert_eq!(
                authkeys::outside_block(&now).unwrap(),
                expected_outside,
                "outside changed for {foreign:?}"
            );
            assert_eq!(mode(&fx.ak()), 0o600);
        }
    }
}

#[test]
fn bak_holds_the_previous_content_and_no_op_edits_write_nothing() {
    let fx = fixture();
    fx.port
        .update("tester", &mut set_block(vec![key(1, "a")]))
        .unwrap();
    let first = std::fs::read(fx.ak()).unwrap();
    let r = fx
        .port
        .update("tester", &mut set_block(vec![key(1, "a"), key(2, "b")]))
        .unwrap();
    let bak = r.backup_path.expect("bak path");
    assert_eq!(std::fs::read(&bak).unwrap(), first);
    assert_eq!(mode(&bak), 0o600);
    let mtime = std::fs::metadata(fx.ak()).unwrap().modified().unwrap();
    let r = fx
        .port
        .update("tester", &mut set_block(vec![key(1, "a"), key(2, "b")]))
        .unwrap();
    assert!(!r.changed && r.backup_path.is_none());
    assert_eq!(
        std::fs::metadata(fx.ak()).unwrap().modified().unwrap(),
        mtime
    );
    // A closure that declines leaves everything alone.
    assert!(!fx.port.update("tester", &mut |_| Ok(None)).unwrap().changed);
}

#[test]
fn ambiguous_markers_abort_without_writing() {
    let fx = fixture();
    std::fs::create_dir_all(fx.home.join(".ssh")).unwrap();
    let broken = format!("{}\n{BEGIN_MARKER}\nstuff\n", key(9, "m"));
    std::fs::write(fx.ak(), &broken).unwrap();
    let e = fx
        .port
        .update("tester", &mut set_block(vec![key(1, "a")]))
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    assert_eq!(std::fs::read_to_string(fx.ak()).unwrap(), broken);
    assert!(!fx.home.join(".ssh/authorized_keys.yanto.bak").exists());
}

#[test]
fn symlinks_and_loose_ssh_dirs_are_refused() {
    let fx = fixture();
    std::fs::create_dir_all(fx.home.join(".ssh")).unwrap();
    let target = fx.home.join("real_keys");
    std::fs::write(&target, "precious\n").unwrap();
    symlink(&target, fx.ak()).unwrap();
    assert!(
        fx.port
            .update("tester", &mut set_block(vec![key(1, "a")]))
            .is_err()
    );
    assert!(fx.port.read("tester").is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious\n");
    std::fs::remove_file(fx.ak()).unwrap();

    std::fs::set_permissions(fx.home.join(".ssh"), std::fs::Permissions::from_mode(0o770)).unwrap();
    let e = fx
        .port
        .update("tester", &mut set_block(vec![key(1, "a")]))
        .unwrap_err();
    assert!(e.message.contains("StrictModes"), "{}", e.message);
    assert!(!fx.ak().exists());

    // A planted temp/backup-name symlink (the user owns ~/.ssh) is never written through.
    std::fs::set_permissions(fx.home.join(".ssh"), std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::write(fx.ak(), "ssh-ed25519 AAAA mine\n").unwrap();
    for planted in [
        ".authorized_keys.yanto.tmp",
        ".authorized_keys.yanto.bak.tmp",
    ] {
        symlink(&target, fx.home.join(".ssh").join(planted)).unwrap();
    }
    fx.port
        .update("tester", &mut set_block(vec![key(1, "a")]))
        .unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "precious\n");
    assert!(std::fs::read_to_string(fx.ak()).unwrap().contains("mine"));

    // `.ssh` itself a symlink.
    let fx = fixture();
    let real = fx.home.join("elsewhere");
    std::fs::create_dir(&real).unwrap();
    symlink(&real, fx.home.join(".ssh")).unwrap();
    assert!(
        fx.port
            .update("tester", &mut set_block(vec![key(1, "a")]))
            .is_err()
    );
    assert!(std::fs::read_dir(&real).unwrap().next().is_none());
}

#[test]
fn missing_or_homeless_users_are_validation_errors() {
    let fx = fixture();
    for u in ["ghost", "relhome"] {
        let e = fx.port.update(u, &mut set_block(vec![])).unwrap_err();
        assert!(matches!(e.kind, ErrorKind::Validation(_)), "{u}");
        assert!(fx.port.read(u).is_err());
    }
}

#[test]
fn a_manual_edit_during_the_write_is_detected_and_not_clobbered() {
    let fx = fixture();
    fx.port
        .update("tester", &mut set_block(vec![key(1, "a")]))
        .unwrap();
    let manual = format!("{}\n", key(9, "added-by-hand"));

    // One racing edit: the writer retries once and the manual line survives next to the new block.
    let mut raced = false;
    let ak = fx.ak();
    let m = manual.clone();
    let mut edit = set_block(vec![key(1, "a"), key(2, "b")]);
    let r = fx.port.update("tester", &mut |bytes| {
        let out = edit(bytes);
        if !raced {
            raced = true;
            let mut cur = std::fs::read(&ak).unwrap();
            cur.extend_from_slice(m.as_bytes());
            std::fs::write(&ak, cur).unwrap();
        }
        out
    });
    r.unwrap();
    let text = std::fs::read_to_string(fx.ak()).unwrap();
    assert!(
        text.contains("added-by-hand"),
        "manual edit survived: {text}"
    );
    assert!(text.contains(&key(2, "b")));

    // A writer that keeps losing the race gives up with a clear error and keeps the manual edit.
    let ak = fx.ak();
    let mut n = 0u8;
    let e = fx
        .port
        .update("tester", &mut |_| {
            n += 1;
            let mut cur = std::fs::read(&ak).unwrap();
            cur.extend_from_slice(format!("# race {n}\n").as_bytes());
            std::fs::write(&ak, cur).unwrap();
            set_block(vec![key(5, "x")])(&[])
        })
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Conflict);
    assert!(e.message.contains("changed"));
    let text = std::fs::read_to_string(fx.ak()).unwrap();
    assert!(text.contains("# race 2") && text.contains("added-by-hand"));
    assert!(!fx.home.join(".ssh/.authorized_keys.yanto.tmp").exists());
}

#[test]
fn verification_failure_restores_the_previous_file() {
    let fx = fixture();
    fx.port
        .update("tester", &mut set_block(vec![key(1, "a")]))
        .unwrap();
    let before = std::fs::read(fx.ak()).unwrap();
    // The edit lies about what should remain outside the block: verification must catch it.
    let e = fx
        .port
        .update("tester", &mut |bytes| {
            let mut s = authkeys::splice(bytes, &[key(2, "b")]).unwrap();
            s.expected_outside.extend_from_slice(b"phantom\n");
            Ok(Some(KeysEdit {
                bytes: s.bytes,
                expected_outside: s.expected_outside,
            }))
        })
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::Internal);
    assert!(e.message.contains("restored"));
    assert_eq!(std::fs::read(fx.ak()).unwrap(), before);
    assert_eq!(mode(&fx.ak()), 0o600);
}
