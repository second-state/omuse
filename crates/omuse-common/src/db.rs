//! Tiny thread-safe SQLite wrapper (WAL mode, JSON rows) — the `DB` class of `app/common/util.py`.

use std::path::Path;

use anyhow::{Context, Result};
use parking_lot::ReentrantMutex;
use rusqlite::types::{ToSqlOutput, Value as SqlValue, ValueRef};
use rusqlite::{params_from_iter, Connection, OptionalExtension, ToSql};
use serde_json::{Map, Value};
use std::cell::RefCell;

/// One SQLite connection shared by a service (re-entrant lock, like Python's `RLock`).
pub struct Db {
    pub path: String,
    conn: ReentrantMutex<RefCell<Connection>>,
}

/// A parameter for [`Db`] queries: converted from the usual Rust types and from `serde_json::Value`.
#[derive(Debug, Clone)]
pub enum Param {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl ToSql for Param {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(match self {
            Param::Null => ToSqlOutput::Owned(SqlValue::Null),
            Param::Int(i) => ToSqlOutput::Owned(SqlValue::Integer(*i)),
            Param::Real(f) => ToSqlOutput::Owned(SqlValue::Real(*f)),
            Param::Text(s) => ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes())),
            Param::Blob(b) => ToSqlOutput::Borrowed(ValueRef::Blob(b)),
        })
    }
}

macro_rules! from_int {
    ($($t:ty),*) => { $( impl From<$t> for Param { fn from(v: $t) -> Self { Param::Int(v as i64) } } )* };
}
from_int!(i64, i32, u32, u64, usize, isize, i8, u8, i16, u16);

impl From<bool> for Param {
    fn from(v: bool) -> Self {
        Param::Int(v as i64)
    }
}
impl From<f64> for Param {
    fn from(v: f64) -> Self {
        Param::Real(v)
    }
}
impl From<f32> for Param {
    fn from(v: f32) -> Self {
        Param::Real(v as f64)
    }
}
impl From<&str> for Param {
    fn from(v: &str) -> Self {
        Param::Text(v.to_string())
    }
}
impl From<String> for Param {
    fn from(v: String) -> Self {
        Param::Text(v)
    }
}
impl From<&String> for Param {
    fn from(v: &String) -> Self {
        Param::Text(v.clone())
    }
}
impl From<Vec<u8>> for Param {
    fn from(v: Vec<u8>) -> Self {
        Param::Blob(v)
    }
}
impl From<&[u8]> for Param {
    fn from(v: &[u8]) -> Self {
        Param::Blob(v.to_vec())
    }
}
impl<T: Into<Param>> From<Option<T>> for Param {
    fn from(v: Option<T>) -> Self {
        match v {
            Some(x) => x.into(),
            None => Param::Null,
        }
    }
}
/// JSON scalars map to SQL scalars; arrays/objects are stored as JSON text (Python code always
/// called `dumps()` explicitly for those, so prefer that; this is a safety net).
impl From<Value> for Param {
    fn from(v: Value) -> Self {
        match v {
            Value::Null => Param::Null,
            Value::Bool(b) => Param::Int(b as i64),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Param::Int(i)
                } else {
                    Param::Real(n.as_f64().unwrap_or(0.0))
                }
            }
            Value::String(s) => Param::Text(s),
            other => Param::Text(crate::pyjson::dumps(&other)),
        }
    }
}
impl From<&Value> for Param {
    fn from(v: &Value) -> Self {
        v.clone().into()
    }
}

/// Build a parameter list: `params![a, b, c]`.
#[macro_export]
macro_rules! params {
    () => { Vec::<$crate::db::Param>::new() };
    ($($p:expr),+ $(,)?) => { vec![$($crate::db::Param::from($p)),+] };
}

fn row_to_json(row: &rusqlite::Row<'_>, names: &[String]) -> Value {
    let mut m = Map::with_capacity(names.len());
    for (i, name) in names.iter().enumerate() {
        let v = match row.get_ref(i) {
            Ok(ValueRef::Null) | Err(_) => Value::Null,
            Ok(ValueRef::Integer(i)) => Value::from(i),
            Ok(ValueRef::Real(f)) => Value::from(f),
            Ok(ValueRef::Text(t)) => Value::String(String::from_utf8_lossy(t).into_owned()),
            Ok(ValueRef::Blob(b)) => {
                use base64::Engine;
                Value::String(base64::engine::general_purpose::STANDARD.encode(b))
            }
        };
        m.insert(name.clone(), v);
    }
    Value::Object(m)
}

impl Db {
    /// Open (creating the parent directory) with WAL, foreign keys and a 30 s busy timeout.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(dir) = path.parent() {
            if !dir.as_os_str().is_empty() {
                std::fs::create_dir_all(dir).with_context(|| format!("mkdir {}", dir.display()))?;
            }
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=30000;",
        )?;
        Ok(Self {
            path: path.to_string_lossy().into_owned(),
            conn: ReentrantMutex::new(RefCell::new(conn)),
        })
    }

    /// In-memory database (tests).
    pub fn memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        Ok(Self {
            path: ":memory:".into(),
            conn: ReentrantMutex::new(RefCell::new(conn)),
        })
    }

    /// Run a closure with the raw connection (for transactions, prepared statements, backups).
    pub fn with<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.conn.lock();
        let conn = guard.borrow();
        f(&conn)
    }

    /// `executescript`.
    pub fn script(&self, sql: &str) -> Result<()> {
        self.with(|c| Ok(c.execute_batch(sql)?))
    }

    /// Execute one statement; returns the number of rows changed.
    pub fn execute(&self, sql: &str, params: Vec<Param>) -> Result<usize> {
        self.with(|c| Ok(c.execute(sql, params_from_iter(params.iter()))?))
    }

    /// First row as a JSON object, or `None`.
    pub fn one(&self, sql: &str, params: Vec<Param>) -> Result<Option<Value>> {
        self.with(|c| {
            let mut st = c.prepare_cached(sql)?;
            let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
            let r = st
                .query_row(params_from_iter(params.iter()), |row| {
                    Ok(row_to_json(row, &names))
                })
                .optional()?;
            Ok(r)
        })
    }

    /// All rows as JSON objects.
    pub fn all(&self, sql: &str, params: Vec<Param>) -> Result<Vec<Value>> {
        self.with(|c| {
            let mut st = c.prepare_cached(sql)?;
            let names: Vec<String> = st.column_names().iter().map(|s| s.to_string()).collect();
            let rows = st
                .query_map(params_from_iter(params.iter()), |row| {
                    Ok(row_to_json(row, &names))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
    }

    /// A single scalar (first column of the first row), e.g. `SELECT COUNT(*)`.
    pub fn scalar(&self, sql: &str, params: Vec<Param>) -> Result<Value> {
        Ok(self
            .one(sql, params)?
            .and_then(|r| r.as_object().and_then(|o| o.values().next().cloned()))
            .unwrap_or(Value::Null))
    }

    /// `INSERT INTO table (cols) VALUES (?)` from a JSON object; returns `last_insert_rowid`.
    pub fn insert(&self, table: &str, data: &Value) -> Result<i64> {
        let obj = data.as_object().context("insert: object expected")?;
        let cols: Vec<&str> = obj.keys().map(|s| s.as_str()).collect();
        let qs = vec!["?"; cols.len()].join(",");
        let sql = format!("INSERT INTO {} ({}) VALUES ({})", table, cols.join(","), qs);
        let params: Vec<Param> = obj.values().map(Param::from).collect();
        self.with(|c| {
            c.execute(&sql, params_from_iter(params.iter()))?;
            Ok(c.last_insert_rowid())
        })
    }

    /// `UPDATE table SET k=? ... WHERE key=?`.
    pub fn update(
        &self,
        table: &str,
        key: &str,
        key_val: impl Into<Param>,
        data: &Value,
    ) -> Result<usize> {
        let obj = data.as_object().context("update: object expected")?;
        if obj.is_empty() {
            return Ok(0);
        }
        let sets: Vec<String> = obj.keys().map(|k| format!("{}=?", k)).collect();
        let sql = format!("UPDATE {} SET {} WHERE {}=?", table, sets.join(","), key);
        let mut params: Vec<Param> = obj.values().map(Param::from).collect();
        params.push(key_val.into());
        self.execute(&sql, params)
    }

    /// `last_insert_rowid()`.
    pub fn last_rowid(&self) -> i64 {
        let guard = self.conn.lock();
        let conn = guard.borrow();
        conn.last_insert_rowid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip() {
        let db = Db::memory().unwrap();
        db.script("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, data BLOB)")
            .unwrap();
        let id = db.insert("t", &json!({"name": "a", "score": 1.5})).unwrap();
        assert_eq!(id, 1);
        db.update("t", "id", id, &json!({"name": "b"})).unwrap();
        let row = db
            .one("SELECT * FROM t WHERE id=?", params![id])
            .unwrap()
            .unwrap();
        assert_eq!(row["name"], "b");
        assert_eq!(row["score"], 1.5);
        assert_eq!(db.all("SELECT * FROM t", params![]).unwrap().len(), 1);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM t", params![]).unwrap(),
            json!(1)
        );
    }
}
