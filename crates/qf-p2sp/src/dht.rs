//! Kademlia DHT 实现
//!
//! 基于 Kademlia 协议的分布式哈希表,用于 Peer 发现。

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// DHT 节点标识(160-bit)
pub type NodeId = [u8; 20];

/// DHT 节点信息
#[derive(Debug, Clone)]
pub struct DhtNode {
    /// 节点 ID
    pub id: NodeId,
    /// 节点地址
    pub addr: String,
    /// 最后通信时间
    pub last_seen: Instant,
}

impl DhtNode {
    pub fn new(id: NodeId, addr: String) -> Self {
        Self {
            id,
            addr,
            last_seen: Instant::now(),
        }
    }

    /// 节点是否过期(超过 15 分钟未通信)
    pub fn is_stale(&self) -> bool {
        self.last_seen.elapsed() > Duration::from_secs(900)
    }
}

/// Kademlia DHT 网络
pub struct KademliaDht {
    /// 本节点 ID
    local_id: NodeId,
    /// 已知节点
    nodes: HashMap<NodeId, DhtNode>,
    /// 最大节点数
    max_nodes: usize,
}

impl KademliaDht {
    /// 创建新的 DHT 实例
    pub fn new(local_id: NodeId, max_nodes: usize) -> Self {
        Self {
            local_id,
            nodes: HashMap::new(),
            max_nodes,
        }
    }

    /// 添加节点
    pub fn add_node(&mut self, node: DhtNode) {
        if self.nodes.len() >= self.max_nodes && !self.nodes.contains_key(&node.id) {
            // 优先移除过期节点,其次移除最旧节点
            let oldest = self
                .nodes
                .iter()
                .filter(|(_, n)| n.is_stale())
                .min_by_key(|(_, n)| n.last_seen)
                .map(|(k, _)| *k)
                .or_else(|| {
                    self.nodes
                        .iter()
                        .min_by_key(|(_, n)| n.last_seen)
                        .map(|(k, _)| *k)
                });
            if let Some(key) = oldest {
                self.nodes.remove(&key);
            }
        }
        self.nodes.insert(node.id, node);
    }

    /// 获取已知节点数
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 获取本节点 ID
    pub fn local_id(&self) -> &NodeId {
        &self.local_id
    }

    /// 清理过期节点
    pub fn cleanup_stale(&mut self) {
        self.nodes.retain(|_, n| !n.is_stale());
    }

    /// 获取所有活跃节点
    pub fn active_nodes(&self) -> Vec<&DhtNode> {
        self.nodes.values().filter(|n| !n.is_stale()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id_byte: u8) -> DhtNode {
        let mut id = [0u8; 20];
        id[0] = id_byte;
        DhtNode::new(id, format!("192.168.1.{id_byte}:8080"))
    }

    #[test]
    fn test_dht_creation() {
        let dht = KademliaDht::new([0u8; 20], 100);
        assert_eq!(dht.node_count(), 0);
    }

    #[test]
    fn test_add_node() {
        let mut dht = KademliaDht::new([0u8; 20], 100);
        dht.add_node(make_node(1));
        assert_eq!(dht.node_count(), 1);
    }

    #[test]
    fn test_max_nodes() {
        let mut dht = KademliaDht::new([0u8; 20], 2);
        dht.add_node(make_node(1));
        dht.add_node(make_node(2));
        dht.add_node(make_node(3));
        assert!(dht.node_count() <= 2);
    }

    #[test]
    fn test_node_not_stale_initially() {
        let node = make_node(1);
        assert!(!node.is_stale());
    }

    #[test]
    fn test_cleanup_stale() {
        let mut dht = KademliaDht::new([0u8; 20], 100);
        dht.add_node(make_node(1));
        assert_eq!(dht.node_count(), 1);
        dht.cleanup_stale();
        // 刚添加的节点不应过期
        assert_eq!(dht.node_count(), 1);
    }

    #[test]
    fn test_local_id() {
        let id = [1u8; 20];
        let dht = KademliaDht::new(id, 100);
        assert_eq!(dht.local_id(), &id);
    }
}
