#![feature(iterator_try_collect, map_try_insert)]
#![feature(impl_trait_in_assoc_type)]
#![feature(type_alias_impl_trait)]

pub mod core;
pub mod network;
pub mod protocol;
pub mod store;
pub mod storage;
pub mod util;

pub use core::{Odyssey, OdysseyConfig};
