//! 嵌入式 KV 存储
//!
//! 基于文件系统的键值存储,每个键对应一个 JSON 文件。
//! 支持异步读写,适用于持久化下载任务状态和配置。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 嵌入式 KV 存储
///
/// 数据以 JSON 文件形式存储在指定目录下。
/// 键经过安全转换(仅保留字母数字和下划线)后作为文件名。
pub struct KvStore {
    /// 存储目录
    dir: PathBuf,
}

impl KvStore {
    /// 创建或打开一个 KV 存储
    pub fn open(dir: impl AsRef<Path>) -> std::io::Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// 获取存储目录
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// 将键转换为安全的文件名
    fn safe_key(key: &str) -> String {
        key.chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }

    /// 键对应的文件路径
    fn path_for(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{}.json", Self::safe_key(key)))
    }

    /// 存储值
    pub fn put<V: Serialize>(&self, key: &str, value: &V) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(value)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(self.path_for(key), json)
    }

    /// 读取值
    pub fn get<V: for<'de> Deserialize<'de>>(&self, key: &str) -> std::io::Result<Option<V>> {
        let path = self.path_for(key);
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(&path)?;
        let value = serde_json::from_str(&data)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Some(value))
    }

    /// 删除键
    pub fn delete(&self, key: &str) -> std::io::Result<bool> {
        let path = self.path_for(key);
        if path.exists() {
            std::fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// 列出所有键
    pub fn keys(&self) -> std::io::Result<Vec<String>> {
        let mut keys = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(key) = name.strip_suffix(".json") {
                keys.push(key.to_string());
            }
        }
        Ok(keys)
    }

    /// 检查键是否存在
    pub fn contains(&self, key: &str) -> bool {
        self.path_for(key).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_put_and_get() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        store.put("greeting", &"hello".to_string()).unwrap();
        let val: Option<String> = store.get("greeting").unwrap();
        assert_eq!(val, Some("hello".to_string()));
    }

    #[test]
    fn test_get_missing_key() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        let val: Option<String> = store.get("nonexistent").unwrap();
        assert_eq!(val, None);
    }

    #[test]
    fn test_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        store.put("key", &42u32).unwrap();
        assert!(store.delete("key").unwrap());
        assert!(!store.delete("key").unwrap());
    }

    #[test]
    fn test_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        store.put("a", &1).unwrap();
        store.put("b", &2).unwrap();
        store.put("c", &3).unwrap();
        let mut keys = store.keys().unwrap();
        keys.sort();
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_contains() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        assert!(!store.contains("x"));
        store.put("x", &"yes").unwrap();
        assert!(store.contains("x"));
    }

    #[test]
    fn test_overwrite() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        store.put("k", &"v1").unwrap();
        store.put("k", &"v2").unwrap();
        let val: Option<String> = store.get("k").unwrap();
        assert_eq!(val, Some("v2".to_string()));
    }

    #[test]
    fn test_struct_value() {
        let tmp = tempfile::tempdir().unwrap();
        let store = KvStore::open(tmp.path()).unwrap();
        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        struct Config {
            name: String,
            value: u32,
        }
        let cfg = Config {
            name: "test".into(),
            value: 42,
        };
        store.put("config", &cfg).unwrap();
        let loaded: Option<Config> = store.get("config").unwrap();
        assert_eq!(loaded, Some(cfg));
    }
}
