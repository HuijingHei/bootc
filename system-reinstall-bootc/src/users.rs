use anyhow::{Context, Result};
use bootc_utils::CommandRunExt;
use rustix::fs::Uid;
use rustix::process::geteuid;
use rustix::process::getuid;
use rustix::thread::set_thread_res_uid;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt::Display;
use std::fmt::Formatter;
use std::process::Command;
use uzers::os::unix::UserExt;

fn loginctl_users() -> Result<BTreeSet<String>> {
    let loginctl_raw_output = loginctl_run_compat()?;

    loginctl_parse(loginctl_raw_output)
}

/// See [`test::test_parse_lsblk`] for example loginctl output
fn loginctl_parse(users: Value) -> Result<BTreeSet<String>> {
    users
        .as_array()
        .context("loginctl output is not an array")?
        .iter()
        .map(|user_value| {
            user_value
                .as_object()
                .context("user entry is not an object")?
                .get("user")
                .context("user object doesn't have a user field")?
                .as_str()
                .context("user name field is not a string")
                .map(String::from)
        })
        // Artificially add the root user to the list of users as it doesn't always appear in
        // `loginctl list-sessions`
        .chain(std::iter::once(Ok("root".to_string())))
        .collect::<Result<_>>()
        .context("error parsing users")
}

/// Run `loginctl` with some compatibility maneuvers to get JSON output
fn loginctl_run_compat() -> Result<Value> {
    let mut command = Command::new("loginctl");
    command.arg("list-sessions").arg("--output").arg("json");
    let output = command.run_get_output().context("running loginctl")?;
    let users: Value = match serde_json::from_reader(output) {
        Ok(users) => users,
        // Failing to parse means loginctl is not outputting JSON despite `--output`
        // (https://github.com/systemd/systemd/issues/15275), we need to use the `--json` flag
        Err(_err) => Command::new("loginctl")
            .arg("list-sessions")
            .arg("--json")
            .arg("short")
            .run_and_parse_json()
            .context("running loginctl")?,
    };
    Ok(users)
}

struct UidChange {
    uid: Uid,
    euid: Uid,
}

impl UidChange {
    fn new(change_to_uid: Uid) -> Result<Self> {
        let (uid, euid) = (getuid(), geteuid());
        set_thread_res_uid(uid, change_to_uid, euid).context("setting effective uid failed")?;
        Ok(Self { uid, euid })
    }
}

impl Drop for UidChange {
    fn drop(&mut self) {
        set_thread_res_uid(self.uid, self.euid, self.euid).expect("setting effective uid failed");
    }
}

#[derive(Clone, Debug)]
pub(crate) struct UserKeys {
    pub(crate) user: String,
    pub(crate) authorized_keys: String,
    pub(crate) authorized_keys_path: String,
}

impl UserKeys {
    pub(crate) fn num_keys(&self) -> usize {
        self.authorized_keys.lines().count()
    }
}

impl Display for UserKeys {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "User {} ({} authorized keys)",
            self.user,
            self.num_keys()
        )
    }
}

pub(crate) fn get_all_users_keys() -> Result<Vec<UserKeys>> {
    let loginctl_user_names = loginctl_users().context("enumerate users")?;

    let mut all_users_authorized_keys = Vec::new();

    for user_name in loginctl_user_names {
        let user_info = uzers::get_user_by_name(user_name.as_str())
            .context(format!("user {} not found", user_name))?;

        let home_dir = user_info.home_dir();
        let user_authorized_keys_path = home_dir.join(".ssh/authorized_keys");

        if !user_authorized_keys_path.exists() {
            tracing::debug!(
                "Skipping user {} because it doesn't have an SSH authorized_keys file",
                user_info.name().to_string_lossy()
            );
            continue;
        }

        let user_name = user_info
            .name()
            .to_str()
            .context("user name is not valid utf-8")?;

        let user_authorized_keys = {
            // Safety: The UID should be valid because we got it from uzers
            #[allow(unsafe_code)]
            let user_uid = unsafe { Uid::from_raw(user_info.uid()) };

            // Change the effective uid for this scope, to avoid accidentally reading files we
            // shouldn't through symlinks
            let _uid_change = UidChange::new(user_uid)?;

            std::fs::read_to_string(&user_authorized_keys_path)
                .context("Failed to read user's authorized keys")?
        };

        if user_authorized_keys.trim().is_empty() {
            tracing::debug!(
                "Skipping user {} because it has an empty SSH authorized_keys file",
                user_info.name().to_string_lossy()
            );
            continue;
        }

        let user_keys = UserKeys {
            user: user_name.to_string(),
            authorized_keys: user_authorized_keys,
            authorized_keys_path: user_authorized_keys_path
                .to_str()
                .context("user's authorized_keys path is not valid utf-8")?
                .to_string(),
        };

        tracing::debug!(
            "Found user {} with {} SSH authorized_keys",
            user_keys.user,
            user_keys.num_keys()
        );

        all_users_authorized_keys.push(user_keys);
    }

    Ok(all_users_authorized_keys)
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    pub(crate) fn test_parse_lsblk() {
        let fixture = include_str!("../tests/fixtures/loginctl.json");

        let result = loginctl_parse(serde_json::from_str(fixture).unwrap()).unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains("root"));
        assert!(result.contains("foo-doe"));
    }
}
