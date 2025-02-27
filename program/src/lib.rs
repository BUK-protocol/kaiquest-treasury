use borsh::{BorshDeserialize, BorshSchema, BorshSerialize}; // Add BorshSerialize
use solana_program::{
    account_info::{next_account_info, AccountInfo},
    entrypoint,
    entrypoint::ProgramResult,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    program_pack::Pack, // Add this import
    pubkey::Pubkey,
    system_instruction,
    sysvar::{rent::Rent, Sysvar},
};

use spl_token::{instruction as token_instruction, state::Account as TokenAccount};

// Declare program entrypoint
entrypoint!(process_instruction);

#[derive(BorshDeserialize, BorshSchema, Debug)]
enum TreasuryInstruction {
    Initialize,
    Claim { amount: u64 },
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct TreasuryConfig {
    pub owner: Pubkey, // Store the original deployer's key
}

#[derive(BorshSerialize, BorshDeserialize, Debug)]
pub struct TreasuryState {
    pub balance: u64, // Store the treasury balance
}

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    instruction_data: &[u8],
) -> ProgramResult {
    let instruction = TreasuryInstruction::try_from_slice(instruction_data)
        .map_err(|_| ProgramError::InvalidInstructionData)?;

    match instruction {
        TreasuryInstruction::Initialize => process_initialize(program_id, accounts),
        TreasuryInstruction::Claim { amount } => process_claim(program_id, accounts, amount),
    }
}

pub fn process_initialize(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();
    let payer = next_account_info(account_info_iter)?; // Deployer
    let treasury_pda = next_account_info(account_info_iter)?; // Treasury PDA
    let system_program = next_account_info(account_info_iter)?;
    let config_account = next_account_info(account_info_iter)?; // Config PDA storing deployer key

    // ✅ Validate Treasury PDA
    let (expected_treasury_pda, treasury_bump) =
        Pubkey::find_program_address(&[b"treasury"], program_id);
    if expected_treasury_pda != *treasury_pda.key {
        return Err(ProgramError::InvalidSeeds);
    }

    // ✅ Validate Config PDA
    let (expected_config_pda, config_bump) = Pubkey::find_program_address(&[b"config"], program_id);
    if expected_config_pda != *config_account.key {
        return Err(ProgramError::InvalidSeeds);
    }

    let rent = Rent::get()?;

    // 🔹 Check if Config PDA already initialized
    if config_account.lamports() > 0 {
        let config_data = config_account.try_borrow_data()?;
        if config_data.len() >= std::mem::size_of::<TreasuryConfig>() {
            let stored_config = TreasuryConfig::try_from_slice(&config_data)
                .map_err(|_| ProgramError::InvalidAccountData)?;

            return Err(ProgramError::AccountAlreadyInitialized);
        }
    }

    // 🔹 Treasury PDA Initialization
    let treasury_space = 8 + std::mem::size_of::<TreasuryState>(); // Correct struct
    let create_treasury_ix = system_instruction::create_account(
        payer.key,
        treasury_pda.key,
        rent.minimum_balance(treasury_space),
        treasury_space as u64,
        program_id,
    );

    invoke_signed(
        &create_treasury_ix,
        &[payer.clone(), treasury_pda.clone(), system_program.clone()],
        &[&[b"treasury", &[treasury_bump]]], // Treasury PDA Seed
    )?;

    // 🔹 Config PDA Initialization
    let config_space = 8 + std::mem::size_of::<TreasuryConfig>();
    let create_config_ix = system_instruction::create_account(
        payer.key,
        config_account.key,
        rent.minimum_balance(config_space),
        config_space as u64,
        program_id,
    );

    invoke_signed(
        &create_config_ix,
        &[
            payer.clone(),
            config_account.clone(),
            system_program.clone(),
        ],
        &[&[b"config", &[config_bump]]], // Config PDA Seed
    )?;

    // ✅ Store the deployer's public key in Config PDA
    let mut config_data = config_account.try_borrow_mut_data()?;
    let config = TreasuryConfig {
        owner: *payer.key, // Store deployer’s key
    };
    config.serialize(&mut &mut config_data[..])?;

    Ok(())
}

pub fn process_claim(program_id: &Pubkey, accounts: &[AccountInfo], amount: u64) -> ProgramResult {
    let account_info_iter = &mut accounts.iter();

    let user = next_account_info(account_info_iter)?;
    let user_token_account = next_account_info(account_info_iter)?;
    let treasury_token_account = next_account_info(account_info_iter)?;
    let token_program = next_account_info(account_info_iter)?;
    let treasury_pda = next_account_info(account_info_iter)?;
    let owner = next_account_info(account_info_iter)?; // Owner account (must sign)
    let config_account = next_account_info(account_info_iter)?; // Config PDA storing deployer

    // ✅ Verify treasury PDA
    let (expected_treasury_pda, bump_seed) =
        Pubkey::find_program_address(&[b"treasury"], program_id);
    if expected_treasury_pda != *treasury_pda.key {
        return Err(ProgramError::InvalidSeeds);
    }
    // 🔹 Check if Config PDA is already initialized
    if config_account.lamports() == 0 {
        return Err(ProgramError::UninitializedAccount);
    }

    let config_data = config_account.try_borrow_data()?;

    // 🔹 Ensure enough space is available
    if config_data.len() < std::mem::size_of::<TreasuryConfig>() {
        return Err(ProgramError::InvalidAccountData);
    }

    // 🔹 Try parsing the config data safely
    // ✅ Ensure Config PDA is initialized
    if config_account.lamports() == 0 {
        return Err(ProgramError::UninitializedAccount);
    }

    // Get raw data
    let config_data = config_account.try_borrow_data()?;

    // 🔹 Debug: Print raw stored data length

    // ✅ Check if stored data matches expected struct size
    if config_data.len() < std::mem::size_of::<TreasuryConfig>() {
        return Err(ProgramError::InvalidAccountData);
    }

    // 🔹 Try parsing manually before using Borsh deserialization
    let owner_bytes: &[u8; 32] = config_data[..32]
        .try_into()
        .map_err(|_| ProgramError::InvalidAccountData)?;
    let stored_owner = Pubkey::new_from_array(*owner_bytes);

    // ✅ Ensure only the original deployer can execute
    if stored_owner != *owner.key {
        return Err(ProgramError::IllegalOwner);
    }

    // ✅ Ensure owner is a signer
    if !owner.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }

    // ✅ Verify treasury token account's owner is treasury PDA
    let treasury_token_account_data = treasury_token_account.try_borrow_data()?;
    let treasury_token_account_info = TokenAccount::unpack(&treasury_token_account_data)
        .map_err(|_| ProgramError::InvalidAccountData)?;

    if treasury_token_account_info.owner != *treasury_pda.key {
        return Err(ProgramError::IllegalOwner);
    }

    drop(treasury_token_account_data); // ✅ Drop before using treasury_token_account again

    // ✅ Transfer tokens from treasury PDA to user
    let transfer_ix = token_instruction::transfer(
        token_program.key,
        treasury_token_account.key,
        user_token_account.key,
        treasury_pda.key,
        &[],
        amount,
    )?;

    invoke_signed(
        &transfer_ix,
        &[
            treasury_token_account.clone(),
            user_token_account.clone(),
            treasury_pda.clone(),
            token_program.clone(),
        ],
        &[&[b"treasury", &[bump_seed]]],
    )?;

    Ok(())
}
