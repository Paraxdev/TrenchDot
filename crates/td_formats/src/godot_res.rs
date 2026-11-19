//! Reads Godot 4 binary resources (`.res`), plain (`RSRC`) or compressed with zstd (`RSCC`), the way Godot's
//! `ResourceLoaderBinary` does. Nothing here writes them, Godot stays the only writer of its files.
//!
//! A file is a header, a string table, the external and internal resource tables and the internal resources, each a
//! type name and a list of properties. The last internal resource is the main one. Every count and length is checked
//! against the bytes left, since the files come from disk and may be damaged or from a newer Godot.

use std::borrow::Cow;

const ZSTD: u32 = 2;
const FORMATS: std::ops::RangeInclusive<u32> = 4..=6;
const FLAG_NAMED_SCENE_IDS: u32 = 1;
const FLAG_UIDS: u32 = 2;
const FLAG_REAL_T_IS_DOUBLE: u32 = 4;
const FLAG_HAS_SCRIPT_CLASS: u32 = 8;
const RESERVED_FIELDS: usize = 11;
/// The most elements one count may claim, far above the 2048 x 2048 maps of the largest Terrain3D region.
pub const MAX_COUNT: usize = 1 << 26;
/// The most a compressed file may unpack to, four times the largest Terrain3D region.
const MAX_STREAM: usize = 1 << 28;
/// Godot always compresses in 4096 byte blocks, this only keeps a damaged size from asking for much memory.
const MAX_BLOCK: usize = 1 << 24;
const MAX_DEPTH: usize = 64;
/// The most elements the arrays, dictionaries, string lists and properties of one file may hold together. A nil takes 4
/// bytes in the file and many times that as a `Value`, so this keeps a damaged file from asking for gigabytes.
const MAX_VALUES: usize = 1 << 23;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum ResError {
    #[error("not a Godot binary resource")]
    Magic,
    #[error("compression mode {0} is not read, only zstd")]
    Compression(u32),
    #[error("resource format {0} is not read, only the Godot 4 formats 4 to 6")]
    Format(u32),
    #[error("big endian resources are not read")]
    BigEndian,
    #[error("the resource ends early")]
    Truncated,
    #[error("a count of {0} is over the limit")]
    TooMany(u64),
    #[error("values nest deeper than {MAX_DEPTH}")]
    TooDeep,
    #[error("unknown value type {0}")]
    UnknownType(u32),
    #[error("damaged resource: {0}")]
    Damaged(String),
}

type Result<T> = std::result::Result<T, ResError>;

/// A Godot `Variant` as a binary resource stores it. Reals are f64 whatever width the file holds, typed arrays and
/// dictionaries read as plain ones because the file does not keep their type.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    StringName(String),
    NodePath(String),
    Vector2([f64; 2]),
    Vector2i([i32; 2]),
    Rect2([f64; 4]),
    Rect2i([i32; 4]),
    Vector3([f64; 3]),
    Vector3i([i32; 3]),
    Vector4([f64; 4]),
    Vector4i([i32; 4]),
    Plane([f64; 4]),
    Quaternion([f64; 4]),
    Aabb([f64; 6]),
    Basis([f64; 9]),
    Transform2D([f64; 6]),
    /// The basis by rows, then the origin.
    Transform3D([f64; 12]),
    Projection([f64; 16]),
    Color([f32; 4]),
    Rid,
    Callable,
    Signal,
    Object(Object),
    Dictionary(Vec<(Value, Value)>),
    Array(Vec<Value>),
    Bytes(Vec<u8>),
    Int32s(Vec<i32>),
    Int64s(Vec<i64>),
    Float32s(Vec<f32>),
    Float64s(Vec<f64>),
    Strings(Vec<String>),
    Vector2s(Vec<[f64; 2]>),
    Vector3s(Vec<[f64; 3]>),
    Colors(Vec<[f32; 4]>),
    Vector4s(Vec<[f64; 4]>),
}

/// A reference to a resource, by its index in the internal or the external table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Object {
    Empty,
    Internal(usize),
    External(usize),
}

impl Value {
    pub fn as_bool(&self) -> Option<bool> {
        if let Value::Bool(b) = self { Some(*b) } else { None }
    }

    pub fn as_int(&self) -> Option<i64> {
        if let Value::Int(i) = self { Some(*i) } else { None }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) | Value::StringName(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        if let Value::Bytes(b) = self { Some(b) } else { None }
    }

    /// The value under the string `key` of a dictionary.
    pub fn get(&self, key: &str) -> Option<&Value> {
        let Value::Dictionary(entries) = self else { return None };
        entries.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct External {
    pub type_name: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SubResource {
    /// `local://<id>` for a resource built into the file, else the path the file was saved at.
    pub path: String,
    pub type_name: String,
    pub properties: Vec<(String, Value)>,
    /// Why the properties after the last one read could not be read, None when all were.
    pub unread: Option<ResError>,
}

impl SubResource {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.properties.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Resource {
    external: Vec<External>,
    internal: Vec<SubResource>,
    named_ids: bool,
}

impl Resource {
    pub fn main(&self) -> &SubResource {
        self.internal.last().expect("parse refuses a file without internal resources")
    }

    pub fn internal(&self) -> &[SubResource] {
        &self.internal
    }

    pub fn external(&self) -> &[External] {
        &self.external
    }

    /// The resource built into the file that `value` refers to. Files from before named ids refer by the number in
    /// `local://<number>`, later ones by the place in the internal table.
    pub fn resolve(&self, value: &Value) -> Option<&SubResource> {
        let Value::Object(Object::Internal(i)) = value else { return None };
        if self.named_ids {
            return self.internal.get(*i);
        }

        let path = format!("local://{i}");
        self.internal.iter().find(|r| r.path == path)
    }
}

/// Reads a whole `.res` file. The header and tables must be intact, a resource whose properties stop partway keeps the
/// ones before and says why in `unread`.
pub fn parse(bytes: &[u8]) -> Result<Resource> {
    let (stream, start) = match bytes.get(..4) {
        Some(b"RSCC") => (Cow::Owned(decompress(bytes)?), 0),
        Some(b"RSRC") => (Cow::Borrowed(bytes), 4),
        _ => return Err(ResError::Magic),
    };
    let mut r = Reader { bytes: &stream, at: start, real64: false, strings: Vec::new(), values: 0 };
    if r.u32()? != 0 {
        return Err(ResError::BigEndian);
    }

    r.take(12)?;
    let format = r.u32()?;
    if !FORMATS.contains(&format) {
        return Err(ResError::Format(format));
    }

    r.string()?;
    r.take(8)?;
    let flags = r.u32()?;
    r.real64 = flags & FLAG_REAL_T_IS_DOUBLE != 0;
    r.take(8)?;
    if flags & FLAG_HAS_SCRIPT_CLASS != 0 {
        r.string()?;
    }

    r.take(RESERVED_FIELDS * 4)?;
    r.strings = r.many(4, Reader::string)?;
    let external = r.many(8, |r| {
        let type_name = r.string()?;
        let path = r.string()?;
        if flags & FLAG_UIDS != 0 {
            r.take(8)?;
        }

        Ok(External { type_name, path })
    })?;
    let table = r.many(12, |r| Ok((r.string()?, r.u64()?)))?;
    if table.is_empty() {
        return Err(ResError::Damaged("no internal resources".into()));
    }

    let internal = table.into_iter().map(|(path, offset)| r.resource(path, offset)).collect::<Result<_>>()?;
    Ok(Resource { external, internal, named_ids: flags & FLAG_NAMED_SCENE_IDS != 0 })
}

/// The stream a `FileAccessCompressed` holds: the magic, mode, block size, total size, one compressed size per block,
/// then the blocks. Every block unpacks to the block size but the last, which holds the rest.
fn decompress(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut r = Reader { bytes, at: 4, real64: false, strings: Vec::new(), values: 0 };
    let mode = r.u32()?;
    if mode != ZSTD {
        return Err(ResError::Compression(mode));
    }

    let block = r.u32()? as usize;
    let total = r.u32()? as usize;
    if block == 0 || block > MAX_BLOCK {
        return Err(ResError::Damaged(format!("block size {block}")));
    }

    if total > MAX_STREAM {
        return Err(ResError::TooMany(total as u64));
    }

    let blocks = total / block + 1;
    if blocks * 4 > r.left() {
        return Err(ResError::Truncated);
    }

    let sizes: Vec<usize> = (0..blocks).map(|_| r.u32().map(|s| s as usize)).collect::<Result<_>>()?;
    let mut out = Vec::new();
    for (i, size) in sizes.into_iter().enumerate() {
        let want = if i + 1 == blocks { total % block } else { block };
        let raw = zstd::bulk::decompress(r.take(size)?, want).map_err(|e| ResError::Damaged(format!("block {i}: {e}")))?;
        if raw.len() != want {
            return Err(ResError::Damaged(format!("block {i} holds {} bytes, not {want}", raw.len())));
        }

        out.extend_from_slice(&raw);
    }

    Ok(out)
}

mod tag {
    pub const NIL: u32 = 1;
    pub const BOOL: u32 = 2;
    pub const INT: u32 = 3;
    pub const FLOAT: u32 = 4;
    pub const STRING: u32 = 5;
    pub const VECTOR2: u32 = 10;
    pub const RECT2: u32 = 11;
    pub const VECTOR3: u32 = 12;
    pub const PLANE: u32 = 13;
    pub const QUATERNION: u32 = 14;
    pub const AABB: u32 = 15;
    pub const BASIS: u32 = 16;
    pub const TRANSFORM3D: u32 = 17;
    pub const TRANSFORM2D: u32 = 18;
    pub const COLOR: u32 = 20;
    pub const NODE_PATH: u32 = 22;
    pub const RID: u32 = 23;
    pub const OBJECT: u32 = 24;
    pub const DICTIONARY: u32 = 26;
    pub const ARRAY: u32 = 30;
    pub const BYTES: u32 = 31;
    pub const INT32S: u32 = 32;
    pub const FLOAT32S: u32 = 33;
    pub const STRINGS: u32 = 34;
    pub const VECTOR3S: u32 = 35;
    pub const COLORS: u32 = 36;
    pub const VECTOR2S: u32 = 37;
    pub const INT64: u32 = 40;
    pub const DOUBLE: u32 = 41;
    pub const CALLABLE: u32 = 42;
    pub const SIGNAL: u32 = 43;
    pub const STRING_NAME: u32 = 44;
    pub const VECTOR2I: u32 = 45;
    pub const RECT2I: u32 = 46;
    pub const VECTOR3I: u32 = 47;
    pub const INT64S: u32 = 48;
    pub const FLOAT64S: u32 = 49;
    pub const VECTOR4: u32 = 50;
    pub const VECTOR4I: u32 = 51;
    pub const PROJECTION: u32 = 52;
    pub const VECTOR4S: u32 = 53;

    pub const OBJECT_EMPTY: u32 = 0;
    pub const OBJECT_INTERNAL: u32 = 2;
    pub const OBJECT_EXTERNAL: u32 = 3;
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    real64: bool,
    strings: Vec<String>,
    values: usize,
}

impl<'a> Reader<'a> {
    fn left(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|end| *end <= self.bytes.len()).ok_or(ResError::Truncated)?;
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("take returns N bytes"))
    }

    fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_le_bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_le_bytes)
    }

    fn i32(&mut self) -> Result<i32> {
        self.array().map(i32::from_le_bytes)
    }

    fn u64(&mut self) -> Result<u64> {
        self.array().map(u64::from_le_bytes)
    }

    fn i64(&mut self) -> Result<i64> {
        self.array().map(i64::from_le_bytes)
    }

    fn f32(&mut self) -> Result<f32> {
        self.array().map(f32::from_le_bytes)
    }

    fn f64(&mut self) -> Result<f64> {
        self.array().map(f64::from_le_bytes)
    }

    fn real(&mut self) -> Result<f64> {
        if self.real64 { self.f64() } else { self.f32().map(f64::from) }
    }

    fn reals<const N: usize>(&mut self) -> Result<[f64; N]> {
        let mut out = [0.0; N];
        for v in &mut out {
            *v = self.real()?;
        }

        Ok(out)
    }

    fn ints<const N: usize>(&mut self) -> Result<[i32; N]> {
        let mut out = [0; N];
        for v in &mut out {
            *v = self.i32()?;
        }

        Ok(out)
    }

    fn color(&mut self) -> Result<[f32; 4]> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }

    /// A u32 length with its UTF-8 bytes, which end in a zero byte the length counts.
    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        self.text(n)
    }

    fn text(&mut self, n: usize) -> Result<String> {
        let bytes = self.take(n)?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }

    /// A string by its index in the string table, or with the top bit set one stored right here.
    fn name(&mut self) -> Result<String> {
        let id = self.u32()?;
        if id & 0x8000_0000 != 0 {
            return self.text((id & 0x7fff_ffff) as usize);
        }

        self.strings.get(id as usize).cloned().ok_or_else(|| ResError::Damaged(format!("string {id} is not in the table")))
    }

    /// A u32 count of elements that each take at least `size` bytes.
    fn count(&mut self, size: usize) -> Result<usize> {
        let n = self.u32()?;
        self.check(n, size)
    }

    fn check(&self, n: u32, size: usize) -> Result<usize> {
        let n = n as usize;
        if n > MAX_COUNT {
            return Err(ResError::TooMany(n as u64));
        }

        if n * size > self.left() { Err(ResError::Truncated) } else { Ok(n) }
    }

    fn many<T>(&mut self, size: usize, one: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let n = self.count(size)?;
        self.repeat(n, one)
    }

    /// The elements of an array or a dictionary, whose count once had its top bit mark a shared container.
    fn items<T>(&mut self, size: usize, one: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        let n = self.u32()? & 0x7fff_ffff;
        let n = self.check(n, size)?;
        self.spend(n)?;
        self.repeat(n, one)
    }

    fn spend(&mut self, n: usize) -> Result<()> {
        self.values += n;
        if self.values > MAX_VALUES { Err(ResError::TooMany(self.values as u64)) } else { Ok(()) }
    }

    fn repeat<T>(&mut self, n: usize, mut one: impl FnMut(&mut Self) -> Result<T>) -> Result<Vec<T>> {
        (0..n).map(|_| one(self)).collect()
    }

    fn real_size(&self) -> usize {
        if self.real64 { 8 } else { 4 }
    }

    fn resource(&mut self, path: String, offset: u64) -> Result<SubResource> {
        self.at = usize::try_from(offset).ok().filter(|o| *o <= self.bytes.len()).ok_or(ResError::Truncated)?;
        let type_name = self.string()?;
        let n = self.count(8)?;
        self.spend(n)?;
        let mut properties = Vec::new();
        let mut unread = None;
        for _ in 0..n {
            match self.name().and_then(|name| Ok((name, self.value(0)?))) {
                Ok(property) => properties.push(property),
                Err(why) => {
                    unread = Some(why);
                    break;
                }
            }
        }

        Ok(SubResource { path, type_name, properties, unread })
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        if depth > MAX_DEPTH {
            return Err(ResError::TooDeep);
        }

        let real = self.real_size();
        Ok(match self.u32()? {
            tag::NIL => Value::Nil,
            tag::BOOL => Value::Bool(self.u32()? != 0),
            tag::INT => Value::Int(self.i32()?.into()),
            tag::INT64 => Value::Int(self.i64()?),
            tag::FLOAT => Value::Float(self.real()?),
            tag::DOUBLE => Value::Float(self.f64()?),
            tag::STRING => Value::String(self.string()?),
            tag::STRING_NAME => Value::StringName(self.string()?),
            tag::NODE_PATH => Value::NodePath(self.node_path()?),
            tag::VECTOR2 => Value::Vector2(self.reals()?),
            tag::VECTOR2I => Value::Vector2i(self.ints()?),
            tag::RECT2 => Value::Rect2(self.reals()?),
            tag::RECT2I => Value::Rect2i(self.ints()?),
            tag::VECTOR3 => Value::Vector3(self.reals()?),
            tag::VECTOR3I => Value::Vector3i(self.ints()?),
            tag::VECTOR4 => Value::Vector4(self.reals()?),
            tag::VECTOR4I => Value::Vector4i(self.ints()?),
            tag::PLANE => Value::Plane(self.reals()?),
            tag::QUATERNION => Value::Quaternion(self.reals()?),
            tag::AABB => Value::Aabb(self.reals()?),
            tag::BASIS => Value::Basis(self.reals()?),
            tag::TRANSFORM2D => Value::Transform2D(self.reals()?),
            tag::TRANSFORM3D => Value::Transform3D(self.reals()?),
            tag::PROJECTION => Value::Projection(self.reals()?),
            tag::COLOR => Value::Color(self.color()?),
            tag::RID => {
                self.u32()?;
                Value::Rid
            }
            tag::CALLABLE => Value::Callable,
            tag::SIGNAL => Value::Signal,
            tag::OBJECT => Value::Object(match self.u32()? {
                tag::OBJECT_EMPTY => Object::Empty,
                tag::OBJECT_INTERNAL => Object::Internal(self.u32()? as usize),
                tag::OBJECT_EXTERNAL => Object::External(self.u32()? as usize),
                kind => return Err(ResError::Damaged(format!("object kind {kind}"))),
            }),
            tag::DICTIONARY => Value::Dictionary(self.items(8, |r| Ok((r.value(depth + 1)?, r.value(depth + 1)?)))?),
            tag::ARRAY => Value::Array(self.items(4, |r| r.value(depth + 1))?),
            tag::BYTES => {
                let n = self.count(1)?;
                let bytes = self.take(n)?.to_vec();
                self.take((4 - n % 4) % 4)?;
                Value::Bytes(bytes)
            }
            tag::INT32S => Value::Int32s(self.many(4, Self::i32)?),
            tag::INT64S => Value::Int64s(self.many(8, Self::i64)?),
            tag::FLOAT32S => Value::Float32s(self.many(4, Self::f32)?),
            tag::FLOAT64S => Value::Float64s(self.many(8, Self::f64)?),
            tag::STRINGS => {
                let n = self.count(4)?;
                self.spend(n)?;
                Value::Strings(self.repeat(n, Self::string)?)
            }
            tag::VECTOR2S => Value::Vector2s(self.many(2 * real, Self::reals)?),
            tag::VECTOR3S => Value::Vector3s(self.many(3 * real, Self::reals)?),
            tag::VECTOR4S => Value::Vector4s(self.many(4 * real, Self::reals)?),
            tag::COLORS => Value::Colors(self.many(16, Self::color)?),
            other => return Err(ResError::UnknownType(other)),
        })
    }

    /// Two u16 counts, of names and of subnames with the top bit marking an absolute path, then the strings.
    fn node_path(&mut self) -> Result<String> {
        let names = self.u16()?;
        let subnames = self.u16()?;
        let mut path = if subnames & 0x8000 != 0 { "/".to_string() } else { String::new() };
        for i in 0..names {
            if i > 0 {
                path.push('/');
            }

            path.push_str(&self.name()?);
        }

        for _ in 0..subnames & 0x7fff {
            path.push(':');
            path.push_str(&self.name()?);
        }

        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn godot() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../godot")
    }

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(godot().join("tests/terrain3d/res_fixtures").join(name)).unwrap()
    }

    fn values(bytes: &[u8]) -> Resource {
        let res = parse(bytes).unwrap();
        assert!(res.internal().iter().all(|r| r.unread.is_none()), "{:?}", res.internal());
        res
    }

    #[test]
    fn every_value_type_reads_as_godot_saved_it() {
        for file in ["values.res", "values_compressed.res"] {
            let res = values(&fixture(file));
            let main = res.main();
            assert_eq!(main.type_name, "Resource", "{file}");
            let meta = |name: &str| main.get(&format!("metadata/{name}")).unwrap_or_else(|| panic!("{file} has no {name}")).clone();
            let transform = [2.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 4.0, -1.0, 2.0, -3.0];
            let want = [
                ("nil", Value::Array(vec![Value::Nil])),
                ("bool", Value::Bool(true)),
                ("int", Value::Int(-7)),
                ("int64", Value::Int(1 << 40)),
                ("float", Value::Float(1.5)),
                ("double", Value::Float(0.1)),
                ("string", Value::String("Grüße".into())),
                ("string_name", Value::StringName("sn".into())),
                ("node_path", Value::NodePath("/root/a:b:c".into())),
                ("vector2", Value::Vector2([1.0, -2.0])),
                ("vector2i", Value::Vector2i([3, -4])),
                ("rect2", Value::Rect2([1.0, 2.0, 3.0, 4.0])),
                ("rect2i", Value::Rect2i([-1, -2, 3, 4])),
                ("vector3", Value::Vector3([1.0, 2.0, -3.0])),
                ("vector3i", Value::Vector3i([1, 2, -3])),
                ("vector4", Value::Vector4([1.0, 2.0, 3.0, -4.0])),
                ("vector4i", Value::Vector4i([1, 2, 3, -4])),
                ("plane", Value::Plane([0.0, 1.0, 0.0, 2.0])),
                ("quaternion", Value::Quaternion([0.0, 0.0, 0.0, 1.0])),
                ("aabb", Value::Aabb([1.0, 2.0, 3.0, 4.0, 5.0, 6.0])),
                ("basis", Value::Basis([2.0, 0.0, 0.0, 0.0, 3.0, 0.0, 0.0, 0.0, 4.0])),
                ("transform2d", Value::Transform2D([1.0, 0.0, -0.0, 1.0, 5.0, 6.0])),
                ("transform3d", Value::Transform3D(transform)),
                ("color", Value::Color([0.25, 0.5, 0.75, 1.0])),
                (
                    "dictionary",
                    Value::Dictionary(vec![
                        (Value::String("k".into()), Value::Array(vec![Value::Int(1), Value::String("two".into())])),
                        (Value::Int(3), Value::Nil),
                    ]),
                ),
                ("array", Value::Array(vec![Value::Int(1), Value::Array(vec![Value::Int(2), Value::Array(vec![Value::Int(3)])])])),
                ("typed_array", Value::Array(vec![Value::Transform3D(transform)])),
                ("typed_dictionary", Value::Dictionary(vec![(Value::Vector2i([1, -2]), Value::Int(5))])),
                ("empty_array", Value::Array(vec![])),
                ("bytes", Value::Bytes(vec![1, 2, 3])),
                ("int32s", Value::Int32s(vec![1, -2])),
                ("int64s", Value::Int64s(vec![1 << 40, -1])),
                ("float32s", Value::Float32s(vec![1.5, -0.25])),
                ("float64s", Value::Float64s(vec![0.1])),
                ("strings", Value::Strings(vec!["a".into(), "".into(), "bc".into()])),
                ("vector2s", Value::Vector2s(vec![[1.0, 2.0]])),
                ("vector3s", Value::Vector3s(vec![[1.0, 2.0, 3.0], [-1.0, -2.0, -3.0]])),
                ("colors", Value::Colors(vec![[1.0, 0.0, 0.0, 0.5]])),
                ("vector4s", Value::Vector4s(vec![[1.0, 2.0, 3.0, 4.0]])),
            ];
            for (name, value) in want {
                assert_eq!(meta(name), value, "{name} in {file}");
            }

            let Value::Projection(p) = meta("projection") else { panic!("{file}: projection") };
            assert_eq!(p, [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

            let inside = res.resolve(&meta("inside")).unwrap();
            assert_eq!((inside.type_name.as_str(), inside.get("offsets")), ("Gradient", Some(&Value::Float32s(vec![0.25, 0.75]))));
            assert!(inside.path.starts_with("local://Gradient_"), "{}", inside.path);
            let Value::Object(Object::External(i)) = meta("outside") else { panic!("{file}: outside") };
            assert_eq!(res.external()[i], External { type_name: "Gradient".into(), path: "res://tests/terrain3d/res_fixtures/outside.tres".into() });
        }
    }

    #[test]
    fn terrain3d_region_files_read_with_their_maps() {
        let res = parse(&std::fs::read(godot().join("terrain3d_valley/terrain3d_00_00.res")).unwrap()).unwrap();
        let main = res.main();
        assert_eq!(
            (main.type_name.as_str(), main.path.as_str(), main.unread.as_ref()),
            ("Terrain3DRegion", "res://terrain3d_valley/terrain3d_00_00.res", None)
        );
        assert_eq!(main.get("region_size"), Some(&Value::Int(256)));
        assert_eq!(main.get("location"), Some(&Value::Vector2i([0, 0])));
        assert_eq!(main.get("version").and_then(Value::as_float), Some(f64::from(1.1_f32)));
        for (map, format, bytes) in [("height_map", "RFloat", 256 * 256 * 4), ("control_map", "RFloat", 256 * 256 * 4), ("color_map", "RGBA8", 349_524)] {
            let image = res.resolve(main.get(map).unwrap()).unwrap_or_else(|| panic!("{map}"));
            let data = image.get("data").unwrap();
            assert_eq!(image.type_name, "Image");
            assert_eq!((data.get("width").and_then(Value::as_int), data.get("height").and_then(Value::as_int)), (Some(256), Some(256)), "{map}");
            assert_eq!(data.get("format").and_then(Value::as_str), Some(format), "{map}");
            assert_eq!(data.get("data").and_then(Value::as_bytes).map(<[u8]>::len), Some(bytes), "{map}");
        }

        for file in ["terrain3d_00-01.res", "terrain3d_01_00.res", "terrain3d_01-01.res", "terrain3d-01_00.res", "terrain3d-01-01.res"] {
            let res = parse(&std::fs::read(godot().join("terrain3d_valley").join(file)).unwrap()).unwrap();
            assert_eq!(res.main().type_name, "Terrain3DRegion", "{file}");
            assert!(res.main().path.ends_with(file), "{file} holds {}", res.main().path);
        }
    }

    #[test]
    fn a_region_from_terrain3d_1_0_has_no_location() {
        let res = parse(&std::fs::read(godot().join("tests/terrain3d/v1_0_2/terrain3d_02-01.res")).unwrap()).unwrap();
        let main = res.main();
        assert_eq!(main.get("version").and_then(Value::as_float), Some(f64::from(0.93_f32)));
        assert_eq!((main.get("region_size"), main.get("vertex_spacing")), (Some(&Value::Int(64)), Some(&Value::Float(2.0))));
        assert_eq!(main.get("location"), None);
        assert_eq!(res.internal().len(), 4);
    }

    /// Where the string table of values.res starts, after the magic, five header fields, the type `Resource`, the
    /// import offset, flags, uid and the reserved fields.
    const STRINGS_AT: usize = 4 + 5 * 4 + 4 + 9 + 8 + 4 + 8 + RESERVED_FIELDS * 4;

    /// The bytes of values.res with the u32 at `at` replaced.
    fn patched(at: usize, value: u32) -> Vec<u8> {
        let mut bytes = fixture("values.res");
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    fn find(bytes: &[u8], what: &[u8]) -> usize {
        bytes.windows(what.len()).position(|w| w == what).unwrap()
    }

    /// Where the main resource of values.res starts, by the offset after its path in the internal table.
    fn main_at(bytes: &[u8]) -> usize {
        let path = find(bytes, b"local://Resource_");
        let len = u32::from_le_bytes(bytes[path - 4..path].try_into().unwrap()) as usize;
        u64::from_le_bytes(bytes[path + len..path + len + 8].try_into().unwrap()) as usize
    }

    #[test]
    fn a_file_cut_short_before_the_properties_of_its_main_resource_is_refused() {
        let bytes = fixture("values.res");
        let main = main_at(&bytes);
        for len in [4, 10, 30, 60, STRINGS_AT, STRINGS_AT + 6, find(&bytes, b"metadata/nil"), find(&bytes, b"local://Gradient_"), main - 30, main, main + 16] {
            assert_eq!(parse(&bytes[..len]).map(|_| ()), Err(ResError::Truncated), "cut at {len}");
        }

        for len in [0, 3] {
            assert_eq!(parse(&bytes[..len]).map(|_| ()), Err(ResError::Magic), "cut at {len}");
        }
    }

    #[test]
    fn a_file_cut_short_in_its_main_resource_keeps_what_came_before() {
        let bytes = fixture("values.res");
        let cut = find(&bytes, &[20, 0, 0, 0, 0, 0, 0x80, 0x3e]) + 4;
        let res = parse(&bytes[..cut]).unwrap();
        let main = res.main();
        assert_eq!(main.unread, Some(ResError::Truncated));
        assert!(main.get("metadata/bool").is_some() && main.get("metadata/color").is_none(), "{:?}", main.properties);
        assert_eq!(res.internal()[0].unread, None);
    }

    #[test]
    fn damaged_counts_and_types_are_errors_not_panics() {
        assert_eq!(parse(&patched(STRINGS_AT, MAX_COUNT as u32 + 1)), Err(ResError::TooMany(MAX_COUNT as u64 + 1)));
        assert_eq!(parse(&patched(STRINGS_AT, 1_000_000)), Err(ResError::Truncated));
        assert_eq!(parse(&patched(STRINGS_AT + 4, u32::MAX)), Err(ResError::Truncated), "a string length past the end");

        let bytes = fixture("values.res");
        let nil = find(&bytes, &[30, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
        let main = |bytes: &[u8]| parse(bytes).unwrap().main().clone();
        assert_eq!(main(&patched(nil + 4, MAX_COUNT as u32 + 7)).unread, Some(ResError::TooMany(MAX_COUNT as u64 + 7)));
        assert_eq!(main(&patched(nil + 4, 0x8000_0001)).unread, None, "the shared bit of an array count");
        assert_eq!(main(&patched(nil + 8, 99)).unread, Some(ResError::UnknownType(99)));
        let first_name = main_at(&bytes) + 4 + 9 + 4;
        let damaged = main(&patched(first_name, 0x7fff_0000));
        assert!(matches!(damaged.unread, Some(ResError::Damaged(_))) && damaged.properties.is_empty(), "{:?}", damaged.unread);

        assert_eq!(parse(&patched(4, 1)), Err(ResError::BigEndian));
        assert_eq!(parse(&patched(20, 3)), Err(ResError::Format(3)));
        assert_eq!(parse(b"GDSC...."), Err(ResError::Magic));
    }

    #[test]
    fn values_nested_past_the_depth_limit_are_refused() {
        let mut bytes = Vec::new();
        for _ in 0..MAX_DEPTH + 2 {
            bytes.extend_from_slice(&tag::ARRAY.to_le_bytes());
            bytes.extend_from_slice(&1u32.to_le_bytes());
        }

        bytes.extend_from_slice(&tag::NIL.to_le_bytes());
        let from = |at: usize| Reader { bytes: &bytes, at, real64: false, strings: Vec::new(), values: 0 }.value(0);
        assert_eq!(from(0), Err(ResError::TooDeep));
        assert!(from(2 * 8).is_ok(), "a nil inside {MAX_DEPTH} arrays");
    }

    #[test]
    fn values_past_the_budget_of_one_file_are_refused() {
        let words = |words: &[u32]| words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>();
        let array = words(&[tag::ARRAY, 2, tag::NIL, tag::NIL]);
        let strings = words(&[tag::STRINGS, 2, 0, 0]);
        let read = |bytes: &[u8], spent: usize| Reader { bytes, at: 0, real64: false, strings: Vec::new(), values: spent }.value(0);
        assert_eq!(read(&array, MAX_VALUES - 2), Ok(Value::Array(vec![Value::Nil, Value::Nil])));
        assert_eq!(read(&array, MAX_VALUES - 1), Err(ResError::TooMany(MAX_VALUES as u64 + 1)));
        assert_eq!(read(&strings, MAX_VALUES - 2), Ok(Value::Strings(vec![String::new(), String::new()])));
        assert_eq!(read(&strings, MAX_VALUES - 1), Err(ResError::TooMany(MAX_VALUES as u64 + 1)));
    }

    #[test]
    fn a_compressed_file_is_read_only_with_zstd_and_whole_blocks() {
        let bytes = fixture("values_compressed.res");
        let with = |at: usize, value: u32| {
            let mut b = bytes.clone();
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
            parse(&b)
        };
        assert_eq!(with(4, 1), Err(ResError::Compression(1)));
        assert_eq!(with(8, 0), Err(ResError::Damaged("block size 0".into())));
        assert!(matches!(with(8, MAX_BLOCK as u32 + 1), Err(ResError::Damaged(_))));
        assert_eq!(with(8, 1), Err(ResError::Truncated), "a block table longer than the file");
        assert_eq!(with(12, (MAX_STREAM + 1) as u32), Err(ResError::TooMany(MAX_STREAM as u64 + 1)));
        assert!(matches!(with(16, 3), Err(ResError::Damaged(_) | ResError::Truncated)), "a block size off by much");
        assert_eq!(parse(&bytes[..bytes.len() / 2]), Err(ResError::Truncated));
        assert_eq!(parse(&bytes[..10]), Err(ResError::Truncated));
    }

    #[test]
    fn reading_one_file_after_another_keeps_nothing_of_the_first() {
        let valley = parse(&std::fs::read(godot().join("terrain3d_valley/terrain3d_00_00.res")).unwrap()).unwrap();
        let values = parse(&fixture("values.res")).unwrap();
        let again = parse(&std::fs::read(godot().join("terrain3d_valley/terrain3d_00_00.res")).unwrap()).unwrap();
        assert_eq!(valley, again);
        assert!(values.main().get("location").is_none() && valley.main().get("metadata/bool").is_none());
    }
}
