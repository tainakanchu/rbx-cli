// SPDX-License-Identifier: GPL-2.0-or-later
//! JSON Schemas of the protocol, generated from the Rust types. The files
//! under `schema/` are these, checked by a test (regenerate with
//! `RBX_CLI_UPDATE_SCHEMA=1 cargo test schema` or `rbx-cli schema --out schema`).

use schemars::schema_for;

/// `(file name, pretty JSON)` for every schema.
#[must_use]
pub fn all() -> Vec<(&'static str, String)> {
    let render = |schema: schemars::Schema| {
        let mut text = serde_json::to_string_pretty(&schema).unwrap_or_default();
        text.push('\n');
        text
    };
    vec![
        (
            "message.json",
            render(schema_for!(crate::protocol::Message)),
        ),
        (
            "request.usb-export.json",
            render(schema_for!(crate::request::ExportRequest)),
        ),
        (
            "result.usb-export.json",
            render(schema_for!(crate::export::ExportResult)),
        ),
        (
            "result.usb-inspect.json",
            render(schema_for!(crate::usb::InspectResult)),
        ),
        (
            "result.usb-verify.json",
            render(schema_for!(crate::usb::VerifyResult)),
        ),
        (
            "result.devices-list.json",
            render(schema_for!(crate::devices::DevicesResult)),
        ),
        (
            "result.devices-eject.json",
            render(schema_for!(crate::devices::EjectResult)),
        ),
        (
            "result.version.json",
            render(schema_for!(crate::version::VersionInfo)),
        ),
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    #[test]
    fn schema_files_are_up_to_date() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema");
        let update = std::env::var_os("RBX_CLI_UPDATE_SCHEMA").is_some();
        let mut stale = Vec::new();
        for (name, text) in super::all() {
            let path = dir.join(name);
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&path, &text).unwrap();
            } else if std::fs::read_to_string(&path)
                .map(|s| s.replace("\r\n", "\n"))
                .ok()
                .as_deref()
                != Some(text.as_str())
            {
                stale.push(name);
            }
        }
        assert!(
            stale.is_empty(),
            "schema/ is out of date ({stale:?}); run `RBX_CLI_UPDATE_SCHEMA=1 cargo test schema`"
        );
    }
}
