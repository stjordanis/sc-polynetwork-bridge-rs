
#![no_std]

use header::*;
use header::peer_config::*;

use util::*;

use elrond_wasm::imports;

imports!();

#[elrond_wasm_derive::contract(BlockHeaderSyncImpl)]
pub trait BlockHeaderSync {
    #[init]
    fn init(&self) {
        
    }

    // endpoints

    #[endpoint(syncGenesisHeader)]
    fn sync_genesis_header(&self, header: Header) -> SCResult<()> {
        require!(self.is_empty_genesis_header(), "Genesis header already set!");
        require!(!header.consensus_payload.is_some(), "Invalid genesis header!");

        let sc_result = self.update_consensus_peer(&header);
        if sc_result.is_ok() {
            self.set_genesis_header(&header);
            self.store_header(&header);
            
            self.block_header_sync_event(&header);
        }

        sc_result
    }

    #[endpoint(syncBlockHeader)]
    fn sync_block_header(&self, header: Header) -> SCResult<()> {
        
        if self.is_empty_header_by_height(header.chain_id, header.height) {
            match self.verify_header(&header) {
                Ok(()) => {},
                Err(err) => return Err(err)
            };

            let sc_result = self.update_consensus_peer(&header);
            if sc_result.is_ok() {
                self.store_header(&header);
                self.block_header_sync_event(&header);
            }

            return sc_result;
        }

        // if block exists already, no sync needed
        Ok(())
    }

    #[view(getHeaderByHeight)]
    fn get_header_by_height_endpoint(&self, chain_id: u64, height: u32) -> Option<Header> {
        if !self.is_empty_header_by_height(chain_id, height) {
            Some(self.get_header_by_height(chain_id, height))
        }
        else {
            None
        }
    }

    #[view(getHeaderByHash)]
    fn get_header_by_hash_endpoint(&self, chain_id: u64, hash: &H256) -> Option<Header> {
        if !self.is_empty_header_by_hash(chain_id, hash) {
            Some(self.get_header_by_hash(chain_id, hash))
        }
        else {
            None
        }
    }

    #[endpoint(verifyHeader)]
    fn verify_header(&self, header: &Header) -> SCResult<()> {
        let chain_id = header.chain_id;
        let height = header.height;

        let key_height = match self.find_key_height(chain_id, height) {
            Some(k) => k,
            None => return sc_error!("Couldn't find key height!")
        };
        let prev_consensus = self.get_consensus_peers(chain_id, key_height);

        if header.book_keepers.len() * 3 < prev_consensus.len() * 2 {
            return sc_error!("Header bookkeepers num must be > 2/3 of consensus num");
        }

        for bk in &header.book_keepers {
            let key_id = hex_converter::byte_slice_to_hex(bk.as_slice());
            
            // if key doesn't exist, something is wrong
            if !prev_consensus.iter().any(|p| p.id == key_id) {
                return sc_error!("Invalid pubkey!");
            }
        }

        let hashed_header = BoxedBytes::from(self.hash_header(header).as_bytes());

        self.verify_multi_signature(&hashed_header, &header.book_keepers, 
            header.book_keepers.len(), &header.sig_data)
    }

    // private

    fn update_consensus_peer(&self, header: &Header) -> SCResult<()> {
        if let Some(consensus_payload) = &header.consensus_payload {
            if let Some(chain_config) = &consensus_payload.new_chain_config {

                let chain_id = header.chain_id;
                let height = header.height;

                // update key heights
                let mut key_heights = self.get_key_height_list(chain_id);
                key_heights.push(height);
                self.set_key_height_list(chain_id, &key_heights);

                // update consensus peer list
                if !chain_config.peers.is_empty() {
                    self.set_consensus_peers(chain_id, height, &chain_config.peers);
                }
                else {
                    return sc_error!("Consensus peer list is empty!")
                }
            }
        }

        Ok(())
    }

    // header-related

    /// hashed twice, for some reason
    fn hash_header(&self, header: &Header) -> H256 {
        self.sha256(self.sha256(header.get_partial_serialized().as_slice()).as_bytes())
    }

    fn store_header(&self, header: &Header) {
        self.set_header_by_hash(header.chain_id, &header.block_hash, header);
        self.set_header_by_height(header.chain_id, header.height, header);
        self.set_current_height(header.chain_id, header.height);
    }

    // verification-related

    /// _height_ should not be lower than current max (which should be the last element). 
    /// If the list is empty (i.e. None is returned from last()),  
    /// then it means genesis header was not initialized
    fn find_key_height(&self, chain_id: u64, height: u32) -> Option<u32> {
        let key_height_list = self.get_key_height_list(chain_id);
        let last_key_height = key_height_list.last();

        match last_key_height {
            Some(k) => {
                if k > &height {
                    None
                }
                else {
                    Some(*k)
                }
            },
            None => None
        }
    }

    // TO DO: verify function not yet available in API
    fn verify(&self, _data: &BoxedBytes, _key: &PublicKey, _sig: &Signature) -> bool {
        true
    }

    fn verify_multi_signature(&self, data: &BoxedBytes, keys: &[PublicKey], 
        min_sigs: usize, sigs: &[Signature]) -> SCResult<()> {

        if sigs.len() < min_sigs {
            return sc_error!("Not enough signatures!");
        }

        let mut mask = Vec::with_capacity(keys.len());
        mask.resize(keys.len(), false);

        for sig in sigs {
            let mut valid = false;

            for j in 0..keys.len() {
                if mask[j] {
                    continue;
                }
                if self.verify(data, &keys[j], sig) {
                    mask[j] = true;
                    valid = true;

                    break;
                }
            }

            if !valid {
                return sc_error!("Multi-signature verification failed!");
            }
        }

        Ok(())
    }

    // events

    #[event("0x1000000000000000000000000000000000000000000000000000000000000001")]
    fn block_header_sync_event(&self, header: &Header);

    // storage

    // genesis header

    #[storage_get("genesisHeader")]
    fn get_genesis_header(&self) -> Header;

    #[storage_set("genesisHeader")]
    fn set_genesis_header(&self, header: &Header);

    #[storage_is_empty("genesisHeader")]
    fn is_empty_genesis_header(&self) -> bool;

    // header by hash

    #[storage_get("headerByHash")]
    fn get_header_by_hash(&self, chain_id: u64, hash: &H256) -> Header;

    #[storage_set("headerByHash")]
    fn set_header_by_hash(&self, chain_id: u64, hash: &H256, header: &Header);

    #[storage_is_empty("headerByHash")]
    fn is_empty_header_by_hash(&self, chain_id: u64, hash: &H256) -> bool;

    // header by height

    #[storage_get("headerByHeight")]
    fn get_header_by_height(&self, chain_id: u64, height: u32) -> Header;

    #[storage_set("headerByHeight")]
    fn set_header_by_height(&self, chain_id: u64, height: u32, header: &Header);

    #[storage_is_empty("headerByHeight")]
    fn is_empty_header_by_height(&self, chain_id: u64, height: u32) -> bool;

    // current height

    #[storage_get("currentHeight")]
    fn get_current_height(&self, chain_id: u64) -> u32;

    #[storage_set("currentHeight")]
    fn set_current_height(&self, chain_id: u64, height: u32);

    // consensus peers

    #[storage_get("consensusPeers")]
    fn get_consensus_peers(&self, chain_id: u64, height: u32) -> Vec<PeerConfig>;

    #[storage_set("consensusPeers")]
    fn set_consensus_peers(&self, chain_id: u64, height: u32, peers: &[PeerConfig]);

    // key height list

    #[storage_get("keyHeightList")]
    fn get_key_height_list(&self, chain_id: u64) -> Vec<u32>;

    #[storage_set("keyHeightList")]
    fn set_key_height_list(&self, chain_id: u64, list: &[u32]);
}
