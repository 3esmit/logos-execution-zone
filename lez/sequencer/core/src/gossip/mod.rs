//! Sequencer p2p gossip: a libp2p swarm gossiping signed sequencer address
//! announcements on a per-channel GossipSub topic.
//!
//! p2p is a latency optimization, never a source of truth: gossip being
//! down degrades to L1-only behavior, and a gossip failure after startup
//! never halts the node.

pub mod announcement;
pub mod directory;
