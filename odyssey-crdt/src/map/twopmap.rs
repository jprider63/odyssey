use im::{OrdMap, OrdSet};
use serde::de::{MapAccess, Visitor};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use std::fmt::{self, Debug};
use std::marker::PhantomData;
use typeable::Typeable;

use crate::time::CausalState;
use crate::CRDT;

/// Two phase map.
#[derive(Clone, Typeable)]
pub struct TwoPMap<K, V> {
    // JP: Drop `K`?
    map: OrdMap<K, V>,
    tombstones: OrdSet<K>,
}

// TODO: Standardized serialization.
impl<K: Serialize + Ord + Clone, V: Serialize + Clone> Serialize for TwoPMap<K, V> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut s = serializer.serialize_struct("TwoPMap", 2)?;
        s.serialize_field("map", &self.map)?;
        s.serialize_field("tombstones", &self.tombstones)?;
        s.end()
    }
}

impl<'d, K: Clone + Ord + Deserialize<'d>, V: Clone + Deserialize<'d>> Deserialize<'d>
    for TwoPMap<K, V>
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'d>,
    {
        struct SVisitor<K, V>(PhantomData<(K, V)>);

        #[derive(Deserialize)]
        #[serde(field_identifier, rename_all = "lowercase")]
        enum Field {
            Map,
            Tombstones,
        }

        impl<'d, K: Ord + Clone + Deserialize<'d>, V: Clone + Deserialize<'d>> Visitor<'d>
            for SVisitor<K, V>
        {
            type Value = TwoPMap<K, V>;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("struct TwoPMap")
            }

            fn visit_map<M>(self, mut m: M) -> Result<TwoPMap<K, V>, M::Error>
            where
                M: MapAccess<'d>,
            {
                let mut map = None;
                let mut tombstones = None;
                while let Some(key) = m.next_key()? {
                    match key {
                        Field::Map => {
                            if map.is_some() {
                                return Err(serde::de::Error::duplicate_field("map"));
                            }
                            map = Some(m.next_value()?);
                        }
                        Field::Tombstones => {
                            if tombstones.is_some() {
                                return Err(serde::de::Error::duplicate_field("tombstones"));
                            }
                            tombstones = Some(m.next_value()?);
                        }
                    }
                }

                let map = map.ok_or_else(|| serde::de::Error::missing_field("map"))?;
                let tombstones =
                    tombstones.ok_or_else(|| serde::de::Error::missing_field("tombstones"))?;

                Ok(TwoPMap { map, tombstones })
            }
        }

        deserializer.deserialize_struct("TwoPMap", &["map", "tombstones"], SVisitor(PhantomData))
    }
}

impl<K: Ord + Debug, V: Debug> Debug for TwoPMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        self.map.fmt(f)
    }
}

// TODO: Define CBOR properly
#[derive(Debug, Serialize, Deserialize)]
pub enum TwoPMapOp<K, V: CRDT> {
    Insert { value: V },
    Apply { key: K, operation: V::Op },
    Delete { key: K },
}

impl<K, V: CRDT> TwoPMapOp<K, V> {
    fn key<'a>(&'a self, op_time: &'a K) -> &K {
        match self {
            TwoPMapOp::Insert { .. } => op_time,
            TwoPMapOp::Apply { key, .. } => key,
            TwoPMapOp::Delete { key } => key,
        }
    }
}

impl<K: Ord + Clone, V: CRDT<Time = K> + Clone> CRDT for TwoPMap<K, V> {
    type Op = TwoPMapOp<K, V>;
    type Time = V::Time; // JP: Newtype wrap `struct TwoPMapId<V>(V::Time)`?

    fn apply<CS: CausalState<Time = Self::Time>>(
        self,
        st: &CS,
        op_time: Self::Time,
        op: Self::Op,
    ) -> Self {
        // Check if deleted.
        let is_deleted = {
            let key = op.key(&op_time);
            self.tombstones.contains(key)
        };
        if is_deleted {
            self
        } else {
            match op {
                TwoPMapOp::Insert { value } => {
                    let TwoPMap { map, tombstones } = self;
                    let map = map.update_with(op_time, value, |_, _| {
                        unreachable!("Invariant violated. Key already exists in TwoPMap.");
                    });

                    TwoPMap { map, tombstones }
                }
                TwoPMapOp::Apply { key, operation } => {
                    let TwoPMap { map, tombstones } = self;
                    let map = map.alter(|v| {
                        if let Some(v) = v {
                            Some(v.apply(st, op_time, operation))
                        } else {
                            unreachable!("Invariant violated. Key must already exist when applyting an update to a TwoPMap.")
                        }
                    }, key);

                    TwoPMap { map, tombstones }
                }
                TwoPMapOp::Delete { key } => {
                    let TwoPMap { map, tombstones } = self;
                    let map = map.without(&key);
                    let tombstones = tombstones.update(key);

                    TwoPMap { map, tombstones }
                }
            }
        }
    }
}

impl<K: Ord, V: CRDT> TwoPMap<K, V> {
    pub fn new() -> TwoPMap<K, V> {
        TwoPMap {
            map: OrdMap::new(),
            tombstones: OrdSet::new(),
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.map.get(key)
    }

    pub fn iter(&self) -> im::ordmap::Iter<'_, K, V> {
        self.map.iter()
    }

    pub fn insert(value: V) -> TwoPMapOp<K, V> {
        TwoPMapOp::Insert { value }
    }
}
