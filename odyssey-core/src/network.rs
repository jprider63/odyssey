
use crate::store;

pub struct ManagerSettings {
}

/// Network manager.
pub struct Manager {
    // Store peer info and store metadata/raw data?
}

impl Manager {
    pub fn initialize(settings: ManagerSettings) -> Manager {
        // Start listening on port.
        // Store settings.

        Manager {
        }
    }

    // 
    pub fn connectToStore<TypeId, H>(store_metadata: store::MetadataHeader<TypeId, H>) -> Result<(), String> { // TODO: async API that pushes errors, applied operations, connection/peer info, etc to a queue?
        unimplemented!{}
    }

    pub fn connectToStoreById<Id>(store_metadata: Id) -> Result<(), String> { // TODO: async API that pushes errors to a queue?
        // TODO: 
        // Lookup id on DHT to retrieve peers
        // Manage peers
        // Retrieve MetadataHeader if we don't have it.
        //   Validate MetadataHeader (matches id, T's invariants, etc)
        // Sync any data
        // Propagate that data asyncronously
        // Store any updates to the file system
        unimplemented!{}
    }
}


