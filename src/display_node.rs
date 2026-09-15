use std::{cell::RefCell, path::PathBuf};

use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::{display::human_readable_number, node::decode_filetime};

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone)]
pub struct DisplayNode {
    // Note: the order of fields in important here, for PartialEq and PartialOrd
    pub size: u64,
    pub name: PathBuf,
    pub children: Vec<DisplayNode>,
}

impl DisplayNode {
    pub fn num_siblings(&self) -> u64 {
        self.children.len() as u64
    }

    pub fn get_children_from_node(&self, is_reversed: bool) -> impl Iterator<Item = &DisplayNode> {
        // we box to avoid the clippy lint warning
        let out: Box<dyn Iterator<Item = &DisplayNode>> = if is_reversed {
            Box::new(self.children.iter().rev())
        } else {
            Box::new(self.children.iter())
        };
        out
    }
}

// How the `size` field is rendered in JSON output. An enum (rather than a
// format string) makes the choice compile-time exhaustive.
#[derive(Debug, Clone, PartialEq)]
pub enum JsonSizeFormat {
    /// human-readable string via human_readable_number, e.g. "1.7Gi"
    Human(String),
    /// raw integer file count (-f)
    Count,
    /// raw integer unix timestamp (-m); DisplayNode.size holds the
    /// order-preserving encoding, decoded back to i64 here (BUG-10)
    Timestamp,
}

// Only used for -j 'json' flag combined with -o 'output_type' flag
// Used to pass the output_type into the custom Serde serializer
thread_local! {
    pub static OUTPUT_TYPE: RefCell<JsonSizeFormat> =
        const { RefCell::new(JsonSizeFormat::Human(String::new())) };
}

// We need the custom Serialize in case someone uses the -o flag to pass a
// custom output type in (show size in Mb / Gb etc), or -f/-m which need raw
// integers instead of human-readable strings.
// Sadly this also necessitates a global variable OUTPUT_TYPE as we can not
// pass the output_type flag into the serialize method
impl Serialize for DisplayNode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("DisplayNode", 3)?;
        OUTPUT_TYPE.with(|fmt| match &*fmt.borrow() {
            JsonSizeFormat::Timestamp => state.serialize_field("size", &decode_filetime(self.size)),
            JsonSizeFormat::Count => state.serialize_field("size", &self.size),
            JsonSizeFormat::Human(fmt) => {
                state.serialize_field("size", &human_readable_number(self.size, fmt))
            },
        })?;
        // PathBuf serialization fails on non-UTF-8 paths; emit lossy instead
        // (pdu BUG-5 lesson: -j must never panic on invalid UTF-8 names)
        state.serialize_field("name", &self.name.to_string_lossy())?;
        state.serialize_field("children", &self.children)?;
        state.end()
    }
}

// The non-UTF-8 name construction needs OsStr::from_bytes, unix-only; on
// Windows invalid-UTF-8 OsStr cannot be built portably in tests.
#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::encode_filetime;

    #[cfg(unix)]
    #[test]
    fn json_serialization_is_lossy_for_non_utf8_names() {
        use std::os::unix::ffi::OsStrExt;
        let node = DisplayNode {
            size: 10,
            name: PathBuf::from(std::ffi::OsStr::from_bytes(b"\xffbad")),
            children: vec![],
        };
        // must not panic / error on invalid UTF-8 (pdu BUG-5 lesson)
        let json = serde_json::to_string(&node).expect("lossy serialization");
        assert!(json.contains("name"));
    }

    // -j is a user-facing API: field names, order and size formatting are
    // stable contract. Locks the exact serialized form per JsonSizeFormat.
    #[test]
    fn json_serialization_exact_bytes() {
        let tree = DisplayNode {
            size: 3 * (1_u64 << 30) + (1_u64 << 29),
            name: PathBuf::from("/a"),
            children: vec![DisplayNode {
                size: 4096,
                name: PathBuf::from("/a/b"),
                children: vec![],
            }],
        };

        // Default OUTPUT_TYPE is Human(""): sizes render human-readable
        let json = serde_json::to_string(&tree).unwrap();
        assert_eq!(
            json,
            r#"{"size":"3.5Gi","name":"/a","children":[{"size":"4.0Ki","name":"/a/b","children":[]}]}"#
        );

        // -m: raw decoded integer timestamps; -f: raw counts
        OUTPUT_TYPE.with(|fmt| {
            fmt.replace(JsonSizeFormat::Timestamp);
        });
        let ts_tree = DisplayNode {
            size: encode_filetime(1_788_493_354),
            name: PathBuf::from("/a"),
            children: vec![],
        };
        let json = serde_json::to_string(&ts_tree).unwrap();
        assert_eq!(json, r#"{"size":1788493354,"name":"/a","children":[]}"#);

        OUTPUT_TYPE.with(|fmt| {
            fmt.replace(JsonSizeFormat::Count);
        });
        let json = serde_json::to_string(&tree).unwrap();
        assert_eq!(
            json,
            r#"{"size":3758096384,"name":"/a","children":[{"size":4096,"name":"/a/b","children":[]}]}"#
        );

        // restore the thread-local default for other tests
        OUTPUT_TYPE.with(|fmt| {
            fmt.replace(JsonSizeFormat::Human(String::new()));
        });
    }
}
