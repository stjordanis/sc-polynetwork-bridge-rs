#![no_std]
#![allow(clippy::string_lit_as_bytes)]

use elrond_wasm::{derive_imports, imports, HexCallDataSerializer};

use transaction::TransactionStatus;

imports!();
derive_imports!();

// erd1qqqqqqqqqqqqqqqpqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzllls8a5w6u
const ESDT_SYSTEM_SC_ADDRESS_ARRAY: [u8; 32] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xff,
];

const ESDT_ISSUE_COST: u64 = 5000000000000000000; // 5 eGLD

const ESDT_TRANSFER_STRING: &[u8] = b"ESDTTransfer";
const ESDT_ISSUE_STRING: &[u8] = b"issue";
const ESDT_MINT_STRING: &[u8] = b"mint";
const ESDT_BURN_STRING: &[u8] = b"ESDTBurn";

const WRAPPED_EGLD_DISPLAY_NAME: &[u8] = b"Wrapped eGLD";
const WRAPPED_EGLD_TICKER: &[u8] = b"WEGLD";
const EGLD_DECIMALS: u8 = 18;

#[derive(NestedEncode, NestedDecode, TopEncode, TopDecode)]
pub struct EsdtOperation<BigUint: BigUintApi> {
    name: BoxedBytes,
    token_identifier: BoxedBytes,
    amount: BigUint,
}

#[elrond_wasm_derive::contract(SimpleEsdtImpl)]
pub trait SimpleEsdt {
    #[init]
    fn init(&self, cross_chain_management_address: Address) {
        self.set_cross_chain_management_contract_address(&cross_chain_management_address);
    }

    // endpoints - owner-only

    #[payable]
    fn perform_wrapped_egld_issue(
        &self,
        initial_supply: BigUint,
        #[payment] payment: BigUint,
    ) -> SCResult<()> {
        only_owner!(self, "only owner may call this function");

        require!(
            self.is_empty_wrapped_egld_token_identifier(),
            "wrapped egld was already issued"
        );

        require!(
            payment == BigUint::from(ESDT_ISSUE_COST),
            "Wrong payment, should pay exactly 5 eGLD for ESDT token issue"
        );

        self.issue_esdt_token(
            &BoxedBytes::from(WRAPPED_EGLD_DISPLAY_NAME),
            &BoxedBytes::from(WRAPPED_EGLD_TICKER),
            &initial_supply,
            EGLD_DECIMALS,
        );

        Ok(())
    }

    #[payable]
    #[endpoint(issueEsdtToken)]
    fn issue_esdt_token_endpoint(
        &self,
        token_display_name: BoxedBytes,
        token_ticker: BoxedBytes,
        initial_supply: BigUint,
        num_decimals: u8,
        #[payment] payment: BigUint,
    ) -> SCResult<()> {
        only_owner!(self, "only owner may call this function");

        require!(
            payment == BigUint::from(ESDT_ISSUE_COST),
            "Wrong payment, should pay exactly 5 eGLD for ESDT token issue"
        );

        self.issue_esdt_token(
            &token_display_name,
            &token_ticker,
            &initial_supply,
            num_decimals,
        );

        Ok(())
    }

    fn burn_esdt_token_endpoint(
        &self,
        token_identifier: BoxedBytes,
        amount: BigUint,
    ) -> SCResult<()> {
        only_owner!(self, "only owner may call this function");

        require!(
            amount <= self.get_total_wrapped_remaining(&token_identifier),
            "Can't burn more than total wrapped remaining"
        );

        self.burn_esdt_token(&token_identifier, &amount);

        Ok(())
    }

    // endpoints - CrossChainManagement contract - only

    #[endpoint(transferEsdtToAccount)]
    fn transfer_esdt_to_account_endpoint(
        &self,
        token_identifier: BoxedBytes,
        amount: BigUint,
        to: Address,
        poly_tx_hash: H256,
    ) -> SCResult<()> {
        require!(
            self.get_caller() == self.get_cross_chain_management_contract_address(),
            "Only the cross chain management contract may call this function"
        );

        let tx_status = self.get_tx_status(&poly_tx_hash);
        require!(
            tx_status == TransactionStatus::None || tx_status == TransactionStatus::OutOfFunds,
            "Transaction was already processed"
        );

        let total_wrapped = self.get_total_wrapped_remaining(&token_identifier);
        if total_wrapped < amount {
            if tx_status != TransactionStatus::OutOfFunds {
                self.set_tx_status(&poly_tx_hash, TransactionStatus::OutOfFunds);
            }

            // we can't return SCError here, as that would erase storage changes, i.e. the status set above
            return Ok(());
        }

        // a send_tx can not fail and has no callback, so we preemptively set it as executed
        self.set_tx_status(&poly_tx_hash, TransactionStatus::Executed);

        if token_identifier != self.get_wrapped_egld_token_identifier() {
            self.transfer_esdt_to_account(&token_identifier, &amount, &to);
        } else {
            // automatically unwrap before sending if the token is wrapped eGLD
            self.transfer_egld_to_account(&to, &amount);
        }

        Ok(())
    }

    #[endpoint(transferEsdtToContract)]
    fn transfer_esdt_to_contract_endpoint(
        &self,
        token_identifier: BoxedBytes,
        amount: BigUint,
        to: Address,
        func_name: BoxedBytes,
        args: Vec<BoxedBytes>,
        poly_tx_hash: H256,
    ) -> SCResult<()> {
        require!(
            self.get_caller() == self.get_cross_chain_management_contract_address(),
            "Only the cross chain management contract may call this function"
        );

        let tx_status = self.get_tx_status(&poly_tx_hash);
        require!(
            tx_status == TransactionStatus::None || tx_status == TransactionStatus::OutOfFunds,
            "Transaction was already processed"
        );

        let total_wrapped = self.get_total_wrapped_remaining(&token_identifier);
        if total_wrapped < amount {
            if tx_status != TransactionStatus::OutOfFunds {
                self.set_tx_status(&poly_tx_hash, TransactionStatus::OutOfFunds);
            }

            // we can't return SCError here, as that would erase storage changes, i.e. the status set above
            return Ok(());
        }

        // setting the status to InProgress so it can't be executed multiple times before the async-call callBack is reached
        self.set_tx_status(&poly_tx_hash, TransactionStatus::InProgress);

        // save the poly_tx_hash to be used in the callback
        self.set_temporary_storage_poly_tx_hash(&self.get_tx_hash(), &poly_tx_hash);

        if token_identifier != self.get_wrapped_egld_token_identifier() {
            self.transfer_esdt_to_contract(&token_identifier, &amount, &to, &func_name, &args);
        } else {
            // automatically unwrap before sending if the token is wrapped eGLD
            self.transfer_egld_to_contract(&to, &amount, &func_name, &args);
        }

        Ok(())
    }

    // endpoints

    #[payable]
    #[endpoint(wrapEgld)]
    fn wrap_egld(&self, #[payment] payment: BigUint) -> SCResult<()> {
        require!(payment > 0, "Payment must be more than 0");

        require!(
            !self.is_empty_wrapped_egld_token_identifier(),
            "Wrapped eGLD was not issued yet"
        );

        let wrapped_egld_token_identifier = self.get_wrapped_egld_token_identifier();
        let wrapped_egld_left = self.get_total_wrapped_remaining(&wrapped_egld_token_identifier);

        require!(
            wrapped_egld_left >= payment,
            "Contract does not have enough wrapped eGLD. Please try again once more is minted."
        );

        self.transfer_esdt_to_account(&wrapped_egld_token_identifier, &payment, &self.get_caller());

        Ok(())
    }

    #[endpoint(unwrapEgld)]
    fn unwrap_egld(&self) -> SCResult<()> {
        let esdt_token_name = self.get_esdt_token_identifier_boxed();
        let wrapped_egld_token_identifier = self.get_wrapped_egld_token_identifier();

        require!(
            esdt_token_name == wrapped_egld_token_identifier,
            "Wrong esdt token"
        );

        let wrapped_egld_payment = self.get_esdt_value_big_uint();

        require!(wrapped_egld_payment > 0, "Must pay more than 0 tokens!");
        // this should never happen, but we'll check anyway
        require!(
            wrapped_egld_payment <= self.get_sc_balance(),
            "Contract does not have enough funds"
        );

        self.add_total_wrapped(&wrapped_egld_token_identifier, &wrapped_egld_payment);

        // 1 wrapped eGLD = 1 eGLD, so we pay back the same amount
        self.send_tx(&self.get_caller(), &wrapped_egld_payment, b"unwrapping");

        Ok(())
    }

    #[endpoint(mintEsdtToken)]
    fn mint_esdt_token_endpoint(
        &self,
        token_identifier: BoxedBytes,
        amount: BigUint,
    ) -> SCResult<()> {
        require!(
            self.get_was_token_issued(&token_identifier),
            "Token must be issued first"
        );
        require!(amount > 0, "Amount minted must be more than 0");

        self.mint_esdt_token(&token_identifier, &amount);

        Ok(())
    }

    // private

    fn get_esdt_token_identifier_boxed(&self) -> BoxedBytes {
        BoxedBytes::from(self.get_esdt_token_name())
    }

    fn transfer_esdt_to_account(
        &self,
        token_identifier: &BoxedBytes,
        amount: &BigUint,
        to: &Address,
    ) {
        let mut serializer = HexCallDataSerializer::new(ESDT_TRANSFER_STRING);
        serializer.push_argument_bytes(token_identifier.as_slice());
        serializer.push_argument_bytes(amount.to_bytes_be().as_slice());

        self.substract_total_wrapped(token_identifier, amount);

        self.send_tx(&to, &BigUint::zero(), serializer.as_slice());
    }

    fn transfer_esdt_to_contract(
        &self,
        token_identifier: &BoxedBytes,
        amount: &BigUint,
        to: &Address,
        func_name: &BoxedBytes,
        args: &Vec<BoxedBytes>,
    ) {
        let mut serializer = HexCallDataSerializer::new(ESDT_TRANSFER_STRING);
        serializer.push_argument_bytes(token_identifier.as_slice());
        serializer.push_argument_bytes(amount.to_bytes_be().as_slice());

        serializer.push_argument_bytes(func_name.as_slice());
        for arg in args {
            serializer.push_argument_bytes(arg.as_slice());
        }

        self.substract_total_wrapped(token_identifier, amount);

        self.async_call(to, &BigUint::zero(), serializer.as_slice());
    }

    fn transfer_egld_to_account(&self, to: &Address, amount: &BigUint) {
        self.send_tx(&to, &amount, b"transfer");
    }

    fn transfer_egld_to_contract(
        &self,
        to: &Address,
        amount: &BigUint,
        func_name: &BoxedBytes,
        args: &Vec<BoxedBytes>,
    ) {
        let mut serializer = HexCallDataSerializer::new(func_name.as_slice());

        for arg in args {
            serializer.push_argument_bytes(arg.as_slice());
        }

        self.async_call(to, amount, serializer.as_slice());
    }

    fn add_total_wrapped(&self, esdt_token_name: &BoxedBytes, amount: &BigUint) {
        let mut total_wrapped = self.get_total_wrapped_remaining(esdt_token_name);
        total_wrapped += amount;
        self.set_total_wrapped_remaining(esdt_token_name, &total_wrapped);
    }

    fn substract_total_wrapped(&self, esdt_token_name: &BoxedBytes, amount: &BigUint) {
        let mut total_wrapped = self.get_total_wrapped_remaining(esdt_token_name);
        total_wrapped -= amount;
        self.set_total_wrapped_remaining(esdt_token_name, &total_wrapped);
    }

    fn issue_esdt_token(
        &self,
        token_display_name: &BoxedBytes,
        token_ticker: &BoxedBytes,
        initial_supply: &BigUint,
        num_decimals: u8,
    ) {
        let mut serializer = HexCallDataSerializer::new(ESDT_ISSUE_STRING);

        serializer.push_argument_bytes(token_display_name.as_slice());
        serializer.push_argument_bytes(token_ticker.as_slice());
        serializer.push_argument_bytes(&initial_supply.to_bytes_be());
        serializer.push_argument_bytes(&[num_decimals]);

        serializer.push_argument_bytes(&b"canFreeze"[..]);
        serializer.push_argument_bytes(&b"false"[..]);

        serializer.push_argument_bytes(&b"canWipe"[..]);
        serializer.push_argument_bytes(&b"false"[..]);

        serializer.push_argument_bytes(&b"canPause"[..]);
        serializer.push_argument_bytes(&b"false"[..]);

        serializer.push_argument_bytes(&b"canMint"[..]);
        serializer.push_argument_bytes(&b"true"[..]);

        serializer.push_argument_bytes(&b"canBurn"[..]);
        serializer.push_argument_bytes(&b"true"[..]);

        serializer.push_argument_bytes(&b"canChangeOwner"[..]);
        serializer.push_argument_bytes(&b"false"[..]);

        serializer.push_argument_bytes(&b"canUpgrade"[..]);
        serializer.push_argument_bytes(&b"false"[..]);

        // save data for callback
        self.set_temporary_storage_esdt_operation(
            &self.get_tx_hash(),
            &EsdtOperation {
                name: BoxedBytes::from(ESDT_ISSUE_STRING),
                token_identifier: BoxedBytes::empty(),
                amount: initial_supply.clone(),
            },
        );

        self.async_call(
            &Address::from(ESDT_SYSTEM_SC_ADDRESS_ARRAY),
            &BigUint::from(ESDT_ISSUE_COST),
            serializer.as_slice(),
        );
    }

    fn mint_esdt_token(&self, token_identifier: &BoxedBytes, amount: &BigUint) {
        let mut serializer = HexCallDataSerializer::new(ESDT_MINT_STRING);
        serializer.push_argument_bytes(token_identifier.as_slice());
        serializer.push_argument_bytes(&amount.to_bytes_be());

        // save data for callback
        // save data for callback
        self.set_temporary_storage_esdt_operation(
            &self.get_tx_hash(),
            &EsdtOperation {
                name: BoxedBytes::from(ESDT_MINT_STRING),
                token_identifier: token_identifier.clone(),
                amount: amount.clone(),
            },
        );

        self.async_call(
            &Address::from(ESDT_SYSTEM_SC_ADDRESS_ARRAY),
            &BigUint::zero(),
            serializer.as_slice(),
        );
    }

    fn burn_esdt_token(&self, token_identifier: &BoxedBytes, amount: &BigUint) {
        let mut serializer = HexCallDataSerializer::new(ESDT_BURN_STRING);
        serializer.push_argument_bytes(token_identifier.as_slice());
        serializer.push_argument_bytes(&amount.to_bytes_be());

        // save data for callback
        // save data for callback
        self.set_temporary_storage_esdt_operation(
            &self.get_tx_hash(),
            &EsdtOperation {
                name: BoxedBytes::from(ESDT_BURN_STRING),
                token_identifier: token_identifier.clone(),
                amount: amount.clone(),
            },
        );

        self.async_call(
            &Address::from(ESDT_SYSTEM_SC_ADDRESS_ARRAY),
            &BigUint::zero(),
            serializer.as_slice(),
        );
    }

    // callbacks

    #[callback_raw]
    fn callback_raw(&self, result: Vec<Vec<u8>>) {
        let original_tx_hash = self.get_tx_hash();

        // if this is empty, it means this callBack comes from an issue ESDT call
        if self.is_empty_temporary_storage_poly_tx_hash(&original_tx_hash) {
            // if this is empty, then there is nothing to do in the callback
            if self.is_empty_temporary_storage_esdt_operation(&original_tx_hash) {
                return;
            }

            let esdt_operation = self.get_temporary_storage_esdt_operation(&original_tx_hash);

            if esdt_operation.name.as_slice() == ESDT_ISSUE_STRING {
                self.perform_esdt_issue_callback(&result, &esdt_operation.amount);
            } else if esdt_operation.name.as_slice() == ESDT_MINT_STRING {
                self.perform_esdt_mint_callback(
                    &result,
                    &esdt_operation.token_identifier,
                    &esdt_operation.amount,
                );
            } else if esdt_operation.name.as_slice() == ESDT_BURN_STRING {
                self.perform_esdt_burn_callback(
                    &result,
                    &esdt_operation.token_identifier,
                    &esdt_operation.amount,
                );
            }

            self.clear_temporary_storage_esdt_operation(&original_tx_hash);

            return;
        } else {
            self.perform_dest_smart_contract_async_callback(&result, &original_tx_hash);

            self.clear_temporary_storage_poly_tx_hash(&original_tx_hash);
        }
    }

    fn perform_dest_smart_contract_async_callback(
        &self,
        result: &Vec<Vec<u8>>,
        original_tx_hash: &H256,
    ) {
        let error_code_vec = &result[0];
        let poly_tx_hash = self.get_temporary_storage_poly_tx_hash(&original_tx_hash);

        // Transaction must be in InProgress status, otherwise, something went wrong
        if self.get_tx_status(&poly_tx_hash) == TransactionStatus::InProgress {
            match u32::dep_decode(&mut error_code_vec.as_slice()) {
                core::result::Result::Ok(err_code) => {
                    // error code 0 means success
                    if err_code == 0 {
                        self.set_tx_status(&poly_tx_hash, TransactionStatus::Executed);
                    } else {
                        self.set_tx_status(&poly_tx_hash, TransactionStatus::Rejected);
                    }
                }
                // we should never get a decode error here, but we'll set the tx to rejected if this somehow happens
                core::result::Result::Err(_) => {
                    self.set_tx_status(&poly_tx_hash, TransactionStatus::Rejected);
                }
            }
        }
    }

    fn perform_esdt_issue_callback(&self, result: &Vec<Vec<u8>>, initial_supply: &BigUint) {
        let error_code_vec = &result[0];
        let token_identifier = BoxedBytes::from(result[1].as_slice());

        match u32::dep_decode(&mut error_code_vec.as_slice()) {
            core::result::Result::Ok(err_code) => {
                // error code 0 means success
                if err_code == 0 {
                    self.set_total_wrapped_remaining(&token_identifier, &initial_supply);
                    self.set_was_token_issued(&token_identifier, true);
                }

                // nothing to do in case of error
            }
            // we should never get a decode error here, but either way, nothing to do if this fails
            core::result::Result::Err(_) => {}
        }
    }

    fn perform_esdt_mint_callback(
        &self,
        result: &Vec<Vec<u8>>,
        token_identifier: &BoxedBytes,
        amount: &BigUint,
    ) {
        let error_code_vec = &result[0];

        match u32::dep_decode(&mut error_code_vec.as_slice()) {
            core::result::Result::Ok(err_code) => {
                // error code 0 means success
                if err_code == 0 {
                    let mut total_wrapped_remaining =
                        self.get_total_wrapped_remaining(&token_identifier);
                    total_wrapped_remaining += amount;
                    self.set_total_wrapped_remaining(&token_identifier, &total_wrapped_remaining);
                }

                // nothing to do in case of error
            }
            // we should never get a decode error here, but either way, nothing to do if this fails
            core::result::Result::Err(_) => {}
        }
    }

    fn perform_esdt_burn_callback(
        &self,
        result: &Vec<Vec<u8>>,
        token_identifier: &BoxedBytes,
        amount: &BigUint,
    ) {
        let error_code_vec = &result[0];

        match u32::dep_decode(&mut error_code_vec.as_slice()) {
            core::result::Result::Ok(err_code) => {
                // error code 0 means success
                if err_code == 0 {
                    let mut total_wrapped_remaining =
                        self.get_total_wrapped_remaining(&token_identifier);
                    total_wrapped_remaining -= amount;
                    self.set_total_wrapped_remaining(&token_identifier, &total_wrapped_remaining);
                }

                // nothing to do in case of error
            }
            // we should never get a decode error here, but either way, nothing to do if this fails
            core::result::Result::Err(_) => {}
        }
    }

    // STORAGE

    // 1 eGLD = 1 wrapped eGLD, and they are interchangeable through this contract

    #[view(getWrappedEgldTokenIdentifier)]
    #[storage_get("wrappedEgldTokenIdentifier")]
    fn get_wrapped_egld_token_identifier(&self) -> BoxedBytes;

    #[storage_set("wrappedEgldTokenIdentifier")]
    fn set_wrapped_egld_token_identifier(&self, token_identifier: &BoxedBytes);

    #[storage_is_empty("wrappedEgldTokenIdentifier")]
    fn is_empty_wrapped_egld_token_identifier(&self) -> bool;

    // The total remaining wrapped tokens of each type owned by this SC. Stored so we don't have to query everytime.

    #[view(getTotalWrappedRemaining)]
    #[storage_get("totalWrappedRemaining")]
    fn get_total_wrapped_remaining(&self, token_identifier: &BoxedBytes) -> BigUint;

    #[storage_set("totalWrappedRemaining")]
    fn set_total_wrapped_remaining(&self, token_identifier: &BoxedBytes, total_wrapped: &BigUint);

    // This is used to be able to tell the difference between a totalWrappedRemaining of 0 and a non-issued token
    // storage_is_empty would return 'true' in both cases

    #[view(wasTokenIssued)]
    #[storage_get("wasTokenIssued")]
    fn get_was_token_issued(&self, token_identifier: &BoxedBytes) -> bool;

    #[storage_set("wasTokenIssued")]
    fn set_was_token_issued(&self, token_identifier: &BoxedBytes, was_token_issued: bool);

    // cross chain management

    #[storage_get("crossChainManagementContractAddress")]
    fn get_cross_chain_management_contract_address(&self) -> Address;

    #[storage_set("crossChainManagementContractAddress")]
    fn set_cross_chain_management_contract_address(&self, address: &Address);

    // tx status

    #[view(getTxStatus)]
    #[storage_get("txStatus")]
    fn get_tx_status(&self, poly_tx_hash: &H256) -> TransactionStatus;

    #[storage_set("txStatus")]
    fn set_tx_status(&self, poly_tx_hash: &H256, status: TransactionStatus);

    // ---------- Temporary storage for raw callbacks ----------

    // temporary storage for the poly_tx_hash, which is NOT the same as original_tx_hash
    // original_tx_hash is what you get when you call self.get_tx_hash() in the api
    // poly_tx_hash is the hash of the poly transaction

    #[storage_get("temporaryStoragePolyTxHash")]
    fn get_temporary_storage_poly_tx_hash(&self, original_tx_hash: &H256) -> H256;

    #[storage_set("temporaryStoragePolyTxHash")]
    fn set_temporary_storage_poly_tx_hash(&self, original_tx_hash: &H256, poly_tx_hash: &H256);

    #[storage_clear("temporaryStoragePolyTxHash")]
    fn clear_temporary_storage_poly_tx_hash(&self, original_tx_hash: &H256);

    #[storage_is_empty("temporaryStoragePolyTxHash")]
    fn is_empty_temporary_storage_poly_tx_hash(&self, original_tx_hash: &H256) -> bool;

    // temporary storage for ESDT operations. Used in callback.

    #[storage_get("temporaryStorageEsdtOperation")]
    fn get_temporary_storage_esdt_operation(
        &self,
        original_tx_hash: &H256,
    ) -> EsdtOperation<BigUint>;

    #[storage_set("temporaryStorageEsdtOperation")]
    fn set_temporary_storage_esdt_operation(
        &self,
        original_tx_hash: &H256,
        esdt_operation: &EsdtOperation<BigUint>,
    );

    #[storage_clear("temporaryStorageEsdtOperation")]
    fn clear_temporary_storage_esdt_operation(&self, original_tx_hash: &H256);

    #[storage_is_empty("temporaryStorageEsdtOperation")]
    fn is_empty_temporary_storage_esdt_operation(&self, original_tx_hash: &H256) -> bool;
}