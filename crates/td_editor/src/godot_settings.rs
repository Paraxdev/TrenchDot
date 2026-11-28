//! Reads and sets string settings in a project's `project.godot`, and resolves the `res://` and `uid://` values they hold.

use crate::code_scan::{entry_end, outside_strings};
use std::{
    fs::{self, File},
    io::{BufRead, BufReader, Read},
    path::Path,
};

/// A header or a `key=value` entry of the file, with the byte range it takes up without its trailing whitespace.
struct Item {
    start: usize,
    end: usize,
    kind: Kind,
}

enum Kind {
    Header(String),
    /// The key and the offset of its `=` inside the entry.
    Key(String, usize),
}

fn items(text: &str) -> Vec<Item> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() {
            at += 1;
            continue;
        }

        let next = entry_end(text, at);
        let part = &text[at..next];
        let end = at + part.trim_end().len();
        match bytes[at] {
            b';' | b'#' => {}
            b'[' => {
                let header = part.trim_end().strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(part);
                out.push(Item { start: at, end, kind: Kind::Header(header.trim().to_string()) });
            }
            _ => {
                if let Some(eq) = outside_strings(part).find(|&(_, c)| c == b'=').map(|(i, _)| i) {
                    out.push(Item { start: at, end, kind: Kind::Key(part[..eq].trim().trim_matches('"').to_string(), eq) });
                }
            }
        }

        at = next;
    }

    out
}

/// Where the value ends in `s`, before whitespace and a trailing comment.
fn value_end(s: &str) -> usize {
    let (mut quoted, mut escaped, mut comment, mut end) = (false, false, false, 0);
    for (i, c) in s.bytes().enumerate() {
        if comment {
            comment = c != b'\n';
            continue;
        }

        if quoted {
            match c {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => quoted = false,
                _ => {}
            }

            end = i + 1;
            continue;
        }

        match c {
            b';' | b'#' => comment = true,
            c if c.is_ascii_whitespace() => {}
            c => {
                quoted = c == b'"';
                end = i + 1;
            }
        }
    }

    end
}

fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }

    out.push('"');
    out
}

/// The text of a value that is one string literal, or None for anything else.
fn unquote(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return None,
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                other => out.push(other),
            },
            c => out.push(c),
        }
    }

    Some(out)
}

/// The value of `key` in `[section]`, an empty section name meaning the keys before the first header. A string literal
/// comes back without its quotes and escapes, any other value as written. When a key is set twice the last one counts,
/// as in Godot.
pub fn get(text: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = section.is_empty();
    let mut found = None;
    for item in items(text) {
        match &item.kind {
            Kind::Header(header) => inside = header == section,
            Kind::Key(k, eq) if inside && k == key => {
                let after = &text[item.start + eq + 1..item.end];
                found = Some(after[..value_end(after)].trim_start().to_string());
            }
            Kind::Key(..) => {}
        }
    }

    found.map(|value| unquote(&value).unwrap_or(value))
}

/// `text` with `key` in `[section]` set to the string `value`, every other byte as it was. The key is added at the end
/// of its section, and the section at the end of the file, when they are missing. New lines follow the file's line
/// endings.
pub fn with_setting(text: &str, section: &str, key: &str, value: &str) -> String {
    with_raw(text, section, key, &quote(value))
}

/// [`with_setting`] with `value` written as it is, for a value that is not a string.
fn with_raw(text: &str, section: &str, key: &str, value: &str) -> String {
    let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let line = format!("{key}={value}");
    let mut inside = section.is_empty();
    let mut last_entry = None;
    let mut existing = None;
    let mut has_section = section.is_empty();
    for item in items(text) {
        match &item.kind {
            Kind::Header(header) => {
                inside = header == section;
                has_section |= inside;
                if inside {
                    last_entry = Some(item.end);
                }
            }
            Kind::Key(k, eq) if inside => {
                last_entry = Some(item.end);
                if k == key {
                    let from = item.start + eq + 1;
                    let after = &text[from..item.end];
                    let lead = after.len() - after.trim_start_matches([' ', '\t']).len();
                    existing = Some((from + lead, from + value_end(after).max(lead)));
                }
            }
            Kind::Key(..) => {}
        }
    }

    let mut out = String::with_capacity(text.len() + line.len() + section.len() + 8);
    if let Some((from, to)) = existing {
        out.push_str(&text[..from]);
        out.push_str(value);
        out.push_str(&text[to..]);
    } else if let Some(at) = last_entry {
        out.push_str(&text[..at]);
        out.push_str(nl);
        out.push_str(&line);
        out.push_str(&text[at..]);
    } else if has_section {
        out.push_str(&line);
        out.push_str(nl);
        out.push_str(text);
    } else {
        out.push_str(text);
        if text.is_empty() {
            out.push_str(&format!("[{section}]{nl}{line}{nl}"));
        } else if text.ends_with('\n') {
            if !text.ends_with("\n\n") && !text.ends_with("\r\n\r\n") {
                out.push_str(nl);
            }

            out.push_str(&format!("[{section}]{nl}{line}{nl}"));
        } else {
            out.push_str(&format!("{nl}{nl}[{section}]{nl}{line}"));
        }
    }

    out
}

/// `text` with `key` moved from `[from]` to the end of `[to]`, its value as written. Unchanged when `[from]` lacks the
/// key or `[to]` has it already.
pub fn moved_setting(text: &str, from: &str, to: &str, key: &str) -> String {
    let mut inside = from.is_empty();
    let mut raw = None;
    for item in items(text) {
        match &item.kind {
            Kind::Header(header) => inside = header == from,
            Kind::Key(k, eq) if inside && k == key => {
                let after = &text[item.start + eq + 1..item.end];
                raw = Some(after[..value_end(after)].trim_start().to_string());
            }
            Kind::Key(..) => {}
        }
    }

    match raw {
        Some(raw) if get(text, to, key).is_none() => with_raw(&without_setting(text, from, key), to, key, &raw),
        _ => text.to_string(),
    }
}

/// `text` without `key` in `[section]`, every other byte as it was.
pub fn without_setting(text: &str, section: &str, key: &str) -> String {
    let mut inside = section.is_empty();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for item in items(text) {
        match &item.kind {
            Kind::Header(header) => inside = header == section,
            Kind::Key(k, _) if inside && k == key => {
                let rest = &text[item.end..];
                let end = item.end + rest.len() - rest.trim_start_matches([' ', '\t']).len();
                let end = end + if text[end..].starts_with("\r\n") { 2 } else { usize::from(text[end..].starts_with('\n')) };
                out.push_str(&text[copied..item.start]);
                copied = end;
            }
            Kind::Key(..) => {}
        }
    }

    out.push_str(&text[copied..]);
    out
}

/// The `res://` path a setting value names. A `res://` value comes back as it is, without a look at the disk. A
/// `uid://` value is looked up in the headers of the project's `.tres` files, `addons` included and folders that start
/// with a dot like `.godot` left out, and is None when no file carries it. An empty value is None too, any other value
/// is an error.
pub fn resolve(root: &Path, value: &str) -> Result<Option<String>, String> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }

    if let Some(rel) = value.strip_prefix("res://") {
        if rel.is_empty() || rel.split(['/', '\\']).any(|part| part == "..") {
            return Err(format!("{value} does not name a file inside the project"));
        }

        return Ok(Some(value.to_string()));
    }

    if value.starts_with("uid://") {
        return Ok(find_uid(root, "", value));
    }

    Err(format!("{value} is not a res:// or uid:// path"))
}

fn find_uid(dir: &Path, rel: &str, uid: &str) -> Option<String> {
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else { continue };
        let path = format!("{rel}{name}");
        if kind.is_dir() {
            if name.starts_with('.') {
                continue;
            }

            if let Some(found) = find_uid(&entry.path(), &format!("{path}/"), uid) {
                return Some(found);
            }
        } else if kind.is_file() && name.ends_with(".tres") && header_uid(&entry.path()).as_deref() == Some(uid) {
            return Some(format!("res://{path}"));
        }
    }

    None
}

fn header_uid(path: &Path) -> Option<String> {
    let mut line = String::new();
    BufReader::new(File::open(path).ok()?.take(4096)).read_line(&mut line).ok()?;
    let at = line.find("uid=\"")? + 5;
    Some(line[at..at + line[at..].find('"')?].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "; Engine configuration file.\n\nconfig_version=5\n\n[application]\n\nconfig/name=\"Room\" ; the name\nconfig/features=PackedStringArray(\"4.7\",\n\"Forward Plus\")\nrun/main_scene=\"res://main.tscn\"\n\n[func_godot]\n\ndefault_map_settings=\"res://old.tres\"\n\n[other]\n\ndefault_map_settings=\"res://not_this.tres\"\n";

    fn with(section: &str, key: &str, value: &str) -> String {
        with_setting(FILE, section, key, value)
    }

    #[test]
    fn get_reads_a_key_from_its_own_section_only() {
        assert_eq!(get(FILE, "func_godot", "default_map_settings").as_deref(), Some("res://old.tres"));
        assert_eq!(get(FILE, "other", "default_map_settings").as_deref(), Some("res://not_this.tres"));
        assert_eq!(get(FILE, "application", "default_map_settings"), None);
    }

    #[test]
    fn get_in_a_missing_section_or_for_a_missing_key_is_none() {
        assert_eq!(get(FILE, "trenchdot", "game_config"), None);
        assert_eq!(get(FILE, "func_godot", "game_config"), None);
        assert_eq!(get("", "a", "b"), None);
    }

    #[test]
    fn get_unquotes_strings_and_leaves_other_values_as_written() {
        assert_eq!(get(FILE, "application", "config/name").as_deref(), Some("Room"));
        assert_eq!(get(FILE, "", "config_version").as_deref(), Some("5"));
        assert_eq!(get("[a]\nk=\"x\\\"y\\\\z\"\n", "a", "k").as_deref(), Some("x\"y\\z"));
        assert_eq!(get("[a]\nk=\"a\" \"b\"\n", "a", "k").as_deref(), Some("\"a\" \"b\""));
        assert_eq!(get("[a]\nk=\n", "a", "k").as_deref(), Some(""));
    }

    #[test]
    fn get_reads_a_value_over_several_lines_and_the_key_after_it() {
        assert_eq!(get(FILE, "application", "config/features").as_deref(), Some("PackedStringArray(\"4.7\",\n\"Forward Plus\")"));
        assert_eq!(get(FILE, "application", "run/main_scene").as_deref(), Some("res://main.tscn"));
    }

    #[test]
    fn get_takes_the_last_of_a_key_set_twice_and_ignores_comments() {
        let text = "[a]\nk=\"first\"\n; k=\"comment\"\n[b]\n[a]\nk=\"last\"\n";
        assert_eq!(get(text, "a", "k").as_deref(), Some("last"));
        assert_eq!(get("[a]\n; k=\"c\"\n", "a", "k"), None);
    }

    #[test]
    fn a_new_key_goes_to_the_end_of_its_section() {
        let want = FILE.replace("default_map_settings=\"res://old.tres\"\n", "default_map_settings=\"res://old.tres\"\ngame=\"res://g.tres\"\n");
        assert_eq!(with("func_godot", "game", "res://g.tres"), want);
    }

    #[test]
    fn a_key_added_to_a_section_after_a_multi_line_value_lands_after_it() {
        let want = FILE.replace("\"Forward Plus\")\nrun/main_scene=\"res://main.tscn\"\n", "\"Forward Plus\")\nrun/main_scene=\"res://main.tscn\"\nx=\"y\"\n");
        assert_eq!(with("application", "x", "y"), want);
    }

    #[test]
    fn an_existing_key_is_replaced_in_place() {
        let want = FILE.replace("\"res://old.tres\"", "\"res://new/a b.tres\"");
        assert_eq!(with("func_godot", "default_map_settings", "res://new/a b.tres"), want);
        assert_eq!(
            get(&with("func_godot", "default_map_settings", "res://new/a b.tres"), "other", "default_map_settings").as_deref(),
            Some("res://not_this.tres")
        );
    }

    #[test]
    fn replacing_keeps_the_comment_after_the_value_and_a_spaced_equals() {
        assert_eq!(with("application", "config/name", "Hall"), FILE.replace("\"Room\" ; the name", "\"Hall\" ; the name"));
        assert_eq!(with_setting("[a]\nk =  \"x\"\t# c\n", "a", "k", "y"), "[a]\nk =  \"y\"\t# c\n");
        assert_eq!(with_setting("[a]\nk=\n[b]\n", "a", "k", "y"), "[a]\nk=\"y\"\n[b]\n");
    }

    #[test]
    fn replacing_a_multi_line_value_replaces_all_of_it() {
        let want = FILE.replace("PackedStringArray(\"4.7\",\n\"Forward Plus\")", "\"x\"");
        assert_eq!(with("application", "config/features", "x"), want);
    }

    #[test]
    fn a_missing_section_is_added_at_the_end_after_a_blank_line() {
        assert_eq!(with("trenchdot", "game_config", "res://c.tres"), format!("{FILE}\n[trenchdot]\ngame_config=\"res://c.tres\"\n"));
        let blank = format!("{FILE}\n");
        assert_eq!(with_setting(&blank, "t", "k", "v"), format!("{blank}[t]\nk=\"v\"\n"));
    }

    #[test]
    fn a_file_without_a_trailing_newline_stays_without_one() {
        let text = "[a]\nk=\"1\"";
        assert_eq!(with_setting(text, "a", "j", "2"), "[a]\nk=\"1\"\nj=\"2\"");
        assert_eq!(with_setting(text, "a", "k", "2"), "[a]\nk=\"2\"");
        assert_eq!(with_setting(text, "b", "j", "2"), "[a]\nk=\"1\"\n\n[b]\nj=\"2\"");
        assert_eq!(with_setting("[a]", "a", "j", "2"), "[a]\nj=\"2\"");
    }

    #[test]
    fn crlf_files_keep_crlf_for_every_new_line() {
        let text = "config_version=5\r\n\r\n[a]\r\n\r\nk=\"1\"\r\n";
        assert_eq!(with_setting(text, "a", "j", "2"), "config_version=5\r\n\r\n[a]\r\n\r\nk=\"1\"\r\nj=\"2\"\r\n");
        assert_eq!(with_setting(text, "a", "k", "2"), "config_version=5\r\n\r\n[a]\r\n\r\nk=\"2\"\r\n");
        assert_eq!(with_setting(text, "b", "j", "2"), format!("{text}\r\n[b]\r\nj=\"2\"\r\n"));
        assert_eq!(with_setting("[a]\r\nk=1", "a", "j", "2"), "[a]\r\nk=1\r\nj=\"2\"");
    }

    #[test]
    fn an_empty_file_gets_the_section_and_a_top_level_key_goes_first() {
        assert_eq!(with_setting("", "a", "k", "v"), "[a]\nk=\"v\"\n");
        assert_eq!(with_setting("[a]\nk=1\n", "", "top", "v"), "top=\"v\"\n[a]\nk=1\n");
        assert_eq!(with_setting("x=1\n[a]\n", "", "top", "v"), "x=1\ntop=\"v\"\n[a]\n");
    }

    #[test]
    fn values_with_quotes_and_backslashes_round_trip() {
        let value = "a\"b\\c\nd";
        assert_eq!(get(&with_setting(FILE, "func_godot", "k", value), "func_godot", "k").as_deref(), Some(value));
    }

    #[test]
    fn non_ascii_text_around_a_replaced_value_is_kept() {
        assert_eq!(with_setting("[a]\nn=\"Zimmer\u{e4}\" ; \u{fc}\n", "a", "n", "x"), "[a]\nn=\"x\" ; \u{fc}\n");
    }

    fn write(root: &Path, rel: &str, text: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn tres(uid: &str) -> String {
        format!("[gd_resource type=\"Resource\" script_class=\"X\" format=3 uid=\"{uid}\"]\n\n[resource]\n")
    }

    #[test]
    fn resolve_returns_a_res_path_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(resolve(dir.path(), "res://entities/a.tres"), Ok(Some("res://entities/a.tres".into())));
        assert_eq!(resolve(dir.path(), " res://a.tres "), Ok(Some("res://a.tres".into())));
        assert_eq!(resolve(dir.path(), ""), Ok(None));
    }

    #[test]
    fn resolve_refuses_values_that_are_no_project_path() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["user://a.tres", "C:/a.tres", "entities/a.tres", "res://", "res://../a.tres", "res://a/../../b.tres"] {
            assert!(resolve(dir.path(), bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolve_finds_a_uid_in_the_second_file_of_a_subfolder() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "entities/a.tres", &tres("uid://aaa"));
        write(dir.path(), "entities/deep/b.tres", &tres("uid://bbb"));
        write(dir.path(), "entities/deep/c.tres", &tres("uid://ccc"));
        assert_eq!(resolve(dir.path(), "uid://ccc"), Ok(Some("res://entities/deep/c.tres".into())));
        assert_eq!(resolve(dir.path(), "uid://aaa"), Ok(Some("res://entities/a.tres".into())));
    }

    #[test]
    fn resolve_returns_none_for_an_unknown_uid_and_a_second_project_does_not_leak() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.tres", &tres("uid://aaa"));
        assert_eq!(resolve(dir.path(), "uid://zzz"), Ok(None));
        let other = tempfile::tempdir().unwrap();
        write(other.path(), "b.tres", &tres("uid://bbb"));
        assert_eq!(resolve(other.path(), "uid://aaa"), Ok(None));
        assert_eq!(resolve(other.path(), "uid://bbb"), Ok(Some("res://b.tres".into())));
        assert_eq!(resolve(&dir.path().join("missing"), "uid://aaa"), Ok(None));
    }

    #[test]
    fn resolve_finds_uids_in_addons_and_ignores_dot_folders() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "addons/trenchdot/x.tres", &tres("uid://in_addons"));
        write(dir.path(), ".godot/imported/y.tres", &tres("uid://in_godot"));
        write(dir.path(), "game/.hidden/w.tres", &tres("uid://in_hidden"));
        write(dir.path(), "game/addons/z.tres", &tres("uid://nested"));
        assert_eq!(resolve(dir.path(), "uid://in_addons"), Ok(Some("res://addons/trenchdot/x.tres".into())));
        assert_eq!(resolve(dir.path(), "uid://in_godot"), Ok(None));
        assert_eq!(resolve(dir.path(), "uid://in_hidden"), Ok(None));
        assert_eq!(resolve(dir.path(), "uid://nested"), Ok(Some("res://game/addons/z.tres".into())));
    }

    #[test]
    fn resolve_reads_only_tres_headers_and_matches_the_whole_uid() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "a.tscn", &tres("uid://scene"));
        write(dir.path(), "b.tres", &tres("uid://abcd"));
        write(dir.path(), "c.tres", "not a header\n[resource]\nuid=\"uid://body\"\n");
        assert_eq!(resolve(dir.path(), "uid://scene"), Ok(None));
        assert_eq!(resolve(dir.path(), "uid://abc"), Ok(None));
        assert_eq!(resolve(dir.path(), "uid://body"), Ok(None));
        assert_eq!(resolve(dir.path(), "uid://abcd"), Ok(Some("res://b.tres".into())));
    }

    #[test]
    fn moved_setting_keeps_the_value_as_written() {
        let text = "[func_godot]\n\ndefault_inverse_scale_factor=16.0\nother=1\n\n[trenchdot]\n\ngame_config=\"res://g.tres\"\n";
        let moved = moved_setting(text, "func_godot", "trenchdot", "default_inverse_scale_factor");
        assert_eq!(moved, "[func_godot]\n\nother=1\n\n[trenchdot]\n\ngame_config=\"res://g.tres\"\ndefault_inverse_scale_factor=16.0\n");
        assert_eq!(moved_setting(&moved, "func_godot", "trenchdot", "default_inverse_scale_factor"), moved, "nothing left to move");
        let both = format!("{text}default_inverse_scale_factor=8.0\n");
        assert_eq!(moved_setting(&both, "func_godot", "trenchdot", "default_inverse_scale_factor"), both, "the new section wins");
    }

    #[test]
    fn without_setting_drops_only_that_key() {
        assert_eq!(without_setting(FILE, "func_godot", "default_map_settings"), FILE.replace("default_map_settings=\"res://old.tres\"\n", ""));
        assert_eq!(without_setting(FILE, "func_godot", "missing"), FILE);
        assert_eq!(without_setting("[a]\r\nk=1\r\nj=2\r\n", "a", "k"), "[a]\r\nj=2\r\n");
    }
}
