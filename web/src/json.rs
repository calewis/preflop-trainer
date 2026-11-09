//! A very small JSON writer, enough for the bridge's output.

pub enum V {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<V>),
    Obj(Vec<(String, V)>),
}

impl V {
    pub fn write(&self, out: &mut String) {
        match self {
            V::Null => out.push_str("null"),
            V::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            V::Num(n) => out.push_str(n),
            V::Str(s) => write_string(s, out),
            V::Arr(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    item.write(out);
                }
                out.push(']');
            }
            V::Obj(fields) => {
                out.push('{');
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(key, out);
                    out.push(':');
                    value.write(out);
                }
                out.push('}');
            }
        }
    }

    pub fn into_string(self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

impl From<bool> for V {
    fn from(b: bool) -> V {
        V::Bool(b)
    }
}

impl From<&str> for V {
    fn from(s: &str) -> V {
        V::Str(s.to_string())
    }
}

impl From<String> for V {
    fn from(s: String) -> V {
        V::Str(s)
    }
}

macro_rules! num_from {
    ($($t:ty),*) => {
        $(impl From<$t> for V {
            fn from(n: $t) -> V { V::Num(n.to_string()) }
        })*
    };
}
num_from!(u8, u32, usize);

impl<T: Into<V>> From<Vec<T>> for V {
    fn from(items: Vec<T>) -> V {
        V::Arr(items.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<V>> From<Option<T>> for V {
    fn from(value: Option<T>) -> V {
        value.map_or(V::Null, Into::into)
    }
}

/// `obj!{ "key": value, ... }`, where each value converts into [`V`].
macro_rules! obj {
    ($($key:literal : $value:expr),* $(,)?) => {
        $crate::json::V::Obj(vec![$(($key.to_string(), $crate::json::V::from($value))),*])
    };
}
pub(crate) use obj;
