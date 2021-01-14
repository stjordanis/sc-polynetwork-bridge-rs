#![no_std]
#![allow(clippy::string_lit_as_bytes)]

use elrond_wasm::{imports, HexCallDataSerializer};

use transaction::TransactionStatus;

imports!();

// erd1qqqqqqqqqqqqqqqpqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzllls8a5w6u
const ESDT_SYSTEM_SC_ADDRESS_ARRAY: [u8; 32] = [
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0xff, 0xff,
];

const ESDT_TRANSFER_STRING: &[u8] = b"ESDTTransfer";
const ESDT_ISSUE_STRING: &[u8] = b"issue";
const ESDT_ISSUE_COST: u64 = 5000000000000000000; // 5 eGLD

const WRAPPED_EGLD_DISPLAY_NAME: &[u8] = b"Wrapped eGLD";
const WRAPPED_EGLD_TICKER: &[u8] = b"WEGLD";
const EGLD_DECIMALS: u8 = 18;

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
            self.is_empty_wrapped_egld_token_name(),
            "wrapped egld was already issued"
        );

        require!(
            payment == BigUint::from(ESDT_ISSUE_COST),
            "Wrong payment, should pay exactly 5 eGLD for ESDT token issue"
        );

        self.issue_esdt_token(
            WRAPPED_EGLD_DISPLAY_NAME,
            WRAPPED_EGLD_TICKER,
            &initial_supply,
            EGLD_DECIMALS,
        );

        Ok(())
    }

    // endpoints - CrossChainManagement contract - only

    #[endpoint(transferEsdtToAccount)]
    fn transfer_esdt_to_account_endpoint(
        &self,
        token_name: BoxedBytes,
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

        let total_wrapped = self.get_total_wrapped_remaining(&token_name);
        if total_wrapped < amount {
            if tx_status != TransactionStatus::OutOfFunds {
                self.set_tx_status(&poly_tx_hash, TransactionStatus::OutOfFunds);
            }

            // we can't return SCError here, as that would erase storage changes, i.e. the status set above
            return Ok(());
        }

        // a send_tx can not fail and has no callback, so we preemptively set it as executed
        self.set_tx_status(&poly_tx_hash, TransactionStatus::Executed);

        if token_name != self.get_wrapped_egld_token_name() {
            self.transfer_esdt_to_account(&token_name, &amount, &to);
        } else {
            // automatically unwrap before sending if the token is wrapped eGLD
            self.transfer_egld_to_account(&to, &amount);
        }

        Ok(())
    }

    #[endpoint(transferEsdtToContract)]
    fn transfer_esdt_to_contract_endpoint(
        &self,
        token_name: BoxedBytes,
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

        let total_wrapped = self.get_total_wrapped_remaining(&token_name);
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

        if token_name != self.get_wrapped_egld_token_name() {
            self.transfer_esdt_to_contract(&token_name, &amount, &to, &func_name, &args);
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
            !self.is_empty_wrapped_egld_token_name(),
            "Wrapped eGLD was not issued yet"
        );

        let wrapped_egld_token_name = self.get_wrapped_egld_token_name();
        let wrapped_egld_left = self.get_total_wrapped_remaining(&wrapped_egld_token_name);

        require!(
            wrapped_egld_left >= payment,
            "Contract does not have enough wrapped eGLD. Please try again once more is minted."
        );

        self.transfer_esdt_to_account(&wrapped_egld_token_name, &payment, &self.get_caller());

        Ok(())
    }

    #[endpoint(unwrapEgld)]
    fn unwrap_egld(&self) -> SCResult<()> {
        let esdt_token_name = self.get_esdt_token_name_boxed();
        let wrapped_egld_token_name = self.get_wrapped_egld_token_name();

        require!(
            esdt_token_name == wrapped_egld_token_name,
            "Wrong esdt token"
        );

        let wrapped_egld_payment = self.get_esdt_value_big_uint();

        require!(wrapped_egld_payment > 0, "Must pay more than 0 tokens!");
        // this should never happen, but we'll check anyway
        require!(
            wrapped_egld_payment <= self.get_sc_balance(),
            "Contract does not have enough funds"
        );

        self.add_total_wrapped(&wrapped_egld_token_name, &wrapped_egld_payment);

        // 1 wrapped eGLD = 1 eGLD, so we pay back the same amount
        self.send_tx(&self.get_caller(), &wrapped_egld_payment, b"unwrapping");

        Ok(())
    }

    #[payable]
    #[endpoint(issueEsdtToken)]
    fn issue_esdt_token_endpoint(
        &self,
        token_display_name: &[u8],
        token_ticker: &[u8],
        initial_supply: BigUint,
        num_decimals: u8,
        #[payment] payment: BigUint,
    ) -> SCResult<()> {
        require!(
            payment == BigUint::from(ESDT_ISSUE_COST),
            "Wrong payment, should pay exactly 5 eGLD for ESDT token issue"
        );

        self.issue_esdt_token(token_display_name, token_ticker, &initial_supply, num_decimals);

        Ok(())
    }

    // private

    fn get_esdt_token_name_boxed(&self) -> BoxedBytes {
        BoxedBytes::from(self.get_esdt_token_name())
    }

    fn transfer_esdt_to_account(
        &self,
        esdt_token_name: &BoxedBytes,
        amount: &BigUint,
        to: &Address,
    ) {
        let mut serializer = HexCallDataSerializer::new(ESDT_TRANSFER_STRING);
        serializer.push_argument_bytes(esdt_token_name.as_slice());
        serializer.push_argument_bytes(amount.to_bytes_be().as_slice());

        self.substract_total_wrapped(esdt_token_name, amount);

        self.send_tx(&to, &BigUint::zero(), serializer.as_slice());
    }

    fn transfer_esdt_to_contract(
        &self,
        esdt_token_name: &BoxedBytes,
        amount: &BigUint,
        to: &Address,
        func_name: &BoxedBytes,
        args: &Vec<BoxedBytes>,
    ) {
        let mut serializer = HexCallDataSerializer::new(ESDT_TRANSFER_STRING);
        serializer.push_argument_bytes(esdt_token_name.as_slice());
        serializer.push_argument_bytes(amount.to_bytes_be().as_slice());

        serializer.push_argument_bytes(func_name.as_slice());
        for arg in args {
            serializer.push_argument_bytes(arg.as_slice());
        }

        self.substract_total_wrapped(esdt_token_name, amount);

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
        token_display_name: &[u8],
        token_ticker: &[u8],
        initial_supply: &BigUint,
        num_decimals: u8,
    ) {
        let mut serializer = HexCallDataSerializer::new(ESDT_ISSUE_STRING);

        serializer.push_argument_bytes(token_display_name);
        serializer.push_argument_bytes(token_ticker);
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
        let original_tx_hash = self.get_tx_hash();
        self.set_temporary_storage_esdt_operation(
            &original_tx_hash,
            &BoxedBytes::from(ESDT_ISSUE_STRING),
        );
        self.set_temporary_storage_esdt_amount(&original_tx_hash, &initial_supply);

        self.async_call(
            &Address::from(ESDT_SYSTEM_SC_ADDRESS_ARRAY),
            &BigUint::from(ESDT_ISSUE_COST),
            serializer.as_slice(),
        );
    }

    // callbacks

    #[callback_raw]
    fn callback_raw(&self, result: Vec<Vec<u8>>) {
        let error_code_vec = &result[0];
        let original_tx_hash = self.get_tx_hash();

        // if this is empty, it means this callBack comes from an issue ESDT call
        if self.is_empty_temporary_storage_poly_tx_hash(&original_tx_hash) {
            // if this is empty, then there is nothing to do in the callback
            if self.is_empty_temporary_storage_esdt_operation(&original_tx_hash) {
                return;
            }

            let esdt_operation = self.get_temporary_storage_esdt_operation(&original_tx_hash);
            let token_identifier = &result[1];

            if esdt_operation.as_slice() == ESDT_ISSUE_STRING {
                match u32::dep_decode(&mut error_code_vec.as_slice()) {
                    core::result::Result::Ok(err_code) => {
                        // error code 0 means success
                        if err_code == 0 {
                            let initial_supply =
                                self.get_temporary_storage_esdt_amount(&original_tx_hash);

                            self.set_total_wrapped_remaining(
                                &BoxedBytes::from(token_identifier.as_slice()),
                                &initial_supply,
                            );
                        }

                        // nothing to do in case of error
                    }
                    // we should never get a decode error here, but either way, nothing to do if this fails
                    core::result::Result::Err(_) => {}
                }
            }

            self.clear_temporary_storage_esdt_operation(&original_tx_hash);
            self.clear_temporary_storage_esdt_amount(&original_tx_hash);

            return;
        }

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

        self.clear_temporary_storage_poly_tx_hash(&original_tx_hash);
    }

    // STORAGE

    // 1 eGLD = 1 wrapped eGLD, and they are interchangeable through this contract

    #[view(getWrappedEgldTokenName)]
    #[storage_get("wrappedEgldTokenName")]
    fn get_wrapped_egld_token_name(&self) -> BoxedBytes;

    #[storage_set("wrappedEgldTokenName")]
    fn set_wrapped_egld_token_name(&self, token_name: &BoxedBytes);

    #[storage_is_empty("wrappedEgldTokenName")]
    fn is_empty_wrapped_egld_token_name(&self) -> bool;

    // The total remaining wrapped tokens of each type owned by this SC. Stored so we don't have to query everytime.

    #[view(getTotalWrapped)]
    #[storage_get("totalWrappedRemaining")]
    fn get_total_wrapped_remaining(&self, token_name: &BoxedBytes) -> BigUint;

    #[storage_set("totalWrappedRemaining")]
    fn set_total_wrapped_remaining(&self, token_name: &BoxedBytes, total_wrapped: &BigUint);

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

    // temporary storage for ESDT operations. Used in callback to determine which function was called

    #[storage_get("temporaryStorageEsdtOperation")]
    fn get_temporary_storage_esdt_operation(&self, original_tx_hash: &H256) -> BoxedBytes;

    #[storage_set("temporaryStorageEsdtOperation")]
    fn set_temporary_storage_esdt_operation(
        &self,
        original_tx_hash: &H256,
        esdt_operation: &BoxedBytes,
    );

    #[storage_clear("temporaryStorageEsdtOperation")]
    fn clear_temporary_storage_esdt_operation(&self, original_tx_hash: &H256);

    #[storage_is_empty("temporaryStorageEsdtOperation")]
    fn is_empty_temporary_storage_esdt_operation(&self, original_tx_hash: &H256) -> bool;

    // temporary storage for ESDT operations' value. Used in callback to determine amount issued/minted/burned

    #[storage_get("temporaryStorageEsdtAmount")]
    fn get_temporary_storage_esdt_amount(&self, original_tx_hash: &H256) -> BigUint;

    #[storage_set("temporaryStorageEsdtAmount")]
    fn set_temporary_storage_esdt_amount(&self, original_tx_hash: &H256, esdt_amount: &BigUint);

    #[storage_clear("temporaryStorageEsdtAmount")]
    fn clear_temporary_storage_esdt_amount(&self, original_tx_hash: &H256);
}
