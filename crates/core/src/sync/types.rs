use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_type {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

uuid_type!(LibraryUuid);
uuid_type!(EntityUuid);
uuid_type!(EventUuid);
uuid_type!(DeviceUuid);
uuid_type!(StreamUuid);
uuid_type!(ActorEpochUuid);
uuid_type!(BindingEpochUuid);
uuid_type!(RevisionUuid);
uuid_type!(CheckpointUuid);
uuid_type!(ConflictUuid);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StreamKind {
    Catalog,
    HostMetadata,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Hlc {
    pub wall_ms: u64,
    pub logical: u32,
}

impl Hlc {
    pub fn tick(self, now_ms: u64) -> Self {
        if now_ms > self.wall_ms {
            Self {
                wall_ms: now_ms,
                logical: 0,
            }
        } else {
            Self {
                wall_ms: self.wall_ms,
                logical: self.logical.saturating_add(1),
            }
        }
    }

    pub fn merge(self, remote: Self, now_ms: u64) -> Self {
        let wall_ms = self.wall_ms.max(remote.wall_ms).max(now_ms);
        let logical = if wall_ms == self.wall_ms && wall_ms == remote.wall_ms {
            self.logical.max(remote.logical).saturating_add(1)
        } else if wall_ms == self.wall_ms {
            self.logical.saturating_add(1)
        } else if wall_ms == remote.wall_ms {
            remote.logical.saturating_add(1)
        } else {
            0
        };
        Self { wall_ms, logical }
    }
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.wall_ms, self.logical).cmp(&(other.wall_ms, other.logical))
    }
}
impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ActorId {
    pub device_uuid: DeviceUuid,
    pub stream_uuid: StreamUuid,
    pub actor_epoch: ActorEpochUuid,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Version {
    pub hlc: Hlc,
    pub device_uuid: DeviceUuid,
    pub actor_epoch: ActorEpochUuid,
    pub actor_seq: u64,
    pub event_uuid: EventUuid,
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (
            &self.hlc,
            &self.device_uuid,
            &self.actor_epoch,
            self.actor_seq,
            &self.event_uuid,
        )
            .cmp(&(
                &other.hlc,
                &other.device_uuid,
                &other.actor_epoch,
                other.actor_seq,
                &other.event_uuid,
            ))
    }
}
impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operation {
    SetField {
        field: String,
        value: serde_json::Value,
    },
    SetAdd {
        set: String,
        member_uuid: EntityUuid,
    },
    SetRemove {
        set: String,
        member_uuid: EntityUuid,
        observed_adds: BTreeSet<EventUuid>,
    },
    SetLocation {
        revision_uuid: RevisionUuid,
        parent_uuid: Option<EntityUuid>,
        name: String,
        supersedes: BTreeSet<RevisionUuid>,
    },
    Delete {
        token: EventUuid,
    },
    Restore {
        observed_tokens: BTreeSet<EventUuid>,
    },
    SetContent {
        revision_uuid: RevisionUuid,
        manifest_id: String,
        supersedes: BTreeSet<RevisionUuid>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub wire_version: u16,
    pub envelope_version: u16,
    pub reducer_version: u16,
    pub library_uuid: LibraryUuid,
    pub event_uuid: EventUuid,
    pub entity_uuid: EntityUuid,
    pub stream_uuid: StreamUuid,
    pub stream_kind: StreamKind,
    pub actor: ActorId,
    pub actor_seq: u64,
    pub binding_epoch: BindingEpochUuid,
    pub hlc: Hlc,
    pub operation: Operation,
    #[serde(default)]
    pub base_events: BTreeSet<EventUuid>,
    #[serde(default)]
    pub key_version: u16,
    pub integrity_hash: [u8; 32],
}

impl EventEnvelope {
    pub fn version(&self) -> Version {
        Version {
            hlc: self.hlc,
            device_uuid: self.actor.device_uuid,
            actor_epoch: self.actor.actor_epoch,
            actor_seq: self.actor_seq,
            event_uuid: self.event_uuid,
        }
    }
}
