use crate::error::AuctionError;
use crate::instruction::AuctionInstruction;
use crate::state::Auction;
use solana_program::account_info::{next_account_info, AccountInfo};
use solana_program::clock::Clock;
use solana_program::entrypoint::ProgramResult;
use solana_program::msg;
use solana_program::program::{invoke, invoke_signed};
use solana_program::program_error::ProgramError;
use solana_program::program_pack::{IsInitialized, Pack};
use solana_program::pubkey::Pubkey;
use solana_program::rent::Rent;
use solana_program::sysvar::Sysvar;
use spl_token::state::Account as TokenAccount;
use std::ops::Add;

pub struct Processor;

impl Processor {
    pub fn process(
        program_id: &Pubkey,
        accounts: &[AccountInfo],
        instruction_data: &[u8],
    ) -> ProgramResult {
        let instruction = AuctionInstruction::unpack(instruction_data)?;
        match instruction {
            AuctionInstruction::Exhibit {
                initial_price,
                seconds,
            } => {
                msg!("Initializing Auction...");
                Self::process_exhibit(accounts, initial_price, seconds, program_id)
            }
            AuctionInstruction::Bid { price } => {
                msg!("Placing a Bid in the Auction...");
                Self::process_bid(accounts, price, program_id)
            }
            AuctionInstruction::Cancel {} => {
                msg!("Cancelling the Auction ...");
                Self::process_cancel(accounts, program_id)
            }
            AuctionInstruction::Close {} => {
                msg!("Closing the Auction ...");
                Self::process_close(accounts, program_id)
            }
        }
    }

    fn process_exhibit(
        accounts: &[AccountInfo],
        initial_price: u64,
        auction_duration_sec: u64,
        program_id: &Pubkey,
    ) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let exhibitor_account = next_account_info(account_info_iter)?;

        if !exhibitor_account.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let exhibitor_nft_account = next_account_info(account_info_iter)?;
        let exhibitor_nft_temp_account = next_account_info(account_info_iter)?;
        let exhibitor_ft_receiving_account = next_account_info(account_info_iter)?;

        let escrow_account = next_account_info(account_info_iter)?;
        let sys_var_rent_account = next_account_info(account_info_iter)?;

        let rent = &Rent::from_account_info(sys_var_rent_account)?;
        if !rent.is_exempt(escrow_account.lamports(), escrow_account.data_len()) {
            return Err(AuctionError::NotRentExempt.into());
        }

        let mut auction_info = Auction::unpack_unchecked(&escrow_account.try_borrow_data()?)?;
        if auction_info.is_initialized() {
            return Err(ProgramError::AccountAlreadyInitialized);
        }

        let sys_var_clock_account = next_account_info(account_info_iter)?;
        let clock = &Clock::from_account_info(sys_var_clock_account)?;

        auction_info.is_initialized = true;
        auction_info.exhibitor_pubkey = *exhibitor_account.key;
        auction_info.exhibiting_nft_temp_pubkey = *exhibitor_nft_temp_account.key;
        auction_info.exhibitor_ft_receiving_pubkey = *exhibitor_ft_receiving_account.key;
        auction_info.price = initial_price;
        auction_info.end_at = clock.unix_timestamp.add(auction_duration_sec as i64);
        Auction::pack(auction_info, &mut escrow_account.try_borrow_mut_data()?)?;

        let (pda, _bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);
        let token_program = next_account_info(account_info_iter)?;

        let exhibit_ix = spl_token::instruction::transfer(
            token_program.key,
            exhibitor_nft_account.key,
            exhibitor_nft_temp_account.key,
            exhibitor_account.key,
            &[], // authority_pubkey is default signer when the signer_pubkeys is empty.
            1,
        )?;
        msg!("Transferring the NFT to the Escrow Account...");
        invoke(
            &exhibit_ix,
            &[
                exhibitor_nft_account.clone(),
                exhibitor_nft_temp_account.clone(),
                exhibitor_account.clone(),
                token_program.clone(),
            ],
        )?;

        let owner_change_ix = spl_token::instruction::set_authority(
            token_program.key,
            exhibitor_nft_temp_account.key,
            Some(&pda),
            spl_token::instruction::AuthorityType::AccountOwner,
            exhibitor_account.key,
            &[], // owner_pubkey is default signer when the signer_pubkeys is empty.
        )?;
        msg!("Changing ownership of the token account...");
        invoke(
            &owner_change_ix,
            &[
                exhibitor_nft_temp_account.clone(),
                exhibitor_account.clone(),
                token_program.clone(),
            ],
        )?;
        Ok(())
    }

    fn process_bid(accounts: &[AccountInfo], price: u64, program_id: &Pubkey) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let bidder_account = next_account_info(account_info_iter)?;

        if !bidder_account.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }
        let highest_bidder_account = next_account_info(account_info_iter)?;
        let highest_bidder_ft_temp_account = next_account_info(account_info_iter)?;
        let highest_bidder_ft_returning_account = next_account_info(account_info_iter)?;

        let bidder_ft_temp_account = next_account_info(account_info_iter)?;
        let bidder_ft_account = next_account_info(account_info_iter)?;

        let escrow_account = next_account_info(account_info_iter)?;
        let mut auction_info = Auction::unpack(&escrow_account.try_borrow_data()?)?;

        let sys_var_clock_account = next_account_info(account_info_iter)?;
        let clock = &Clock::from_account_info(sys_var_clock_account)?;

        if auction_info.end_at <= clock.unix_timestamp {
            return Err(AuctionError::InactiveAuction.into());
        }

        if auction_info.price >= price {
            return Err(AuctionError::InsufficientBidPrice.into());
        }

        if auction_info.highest_bidder_ft_temp_pubkey != *highest_bidder_ft_temp_account.key {
            return Err(AuctionError::InvalidInstruction.into());
        }
        if auction_info.highest_bidder_ft_returning_pubkey
            != *highest_bidder_ft_returning_account.key
        {
            return Err(AuctionError::InvalidInstruction.into());
        }
        if auction_info.highest_bidder_pubkey != *highest_bidder_account.key {
            return Err(AuctionError::InvalidInstruction.into());
        }
        if auction_info.highest_bidder_pubkey == *bidder_account.key {
            return Err(AuctionError::AlreadyBid.into());
        }
        let token_program = next_account_info(account_info_iter)?;
        let pda_account = next_account_info(account_info_iter)?;
        let (pda, bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);

        let transfer_to_escrow_ix = spl_token::instruction::transfer(
            token_program.key,
            bidder_ft_account.key,
            bidder_ft_temp_account.key,
            bidder_account.key,
            &[], 
            price,
        )?;
        msg!("Transferring FT to the Escrow Account from the bidder...");
        invoke(
            &transfer_to_escrow_ix,
            &[
                bidder_ft_account.clone(),
                bidder_ft_temp_account.clone(),
                bidder_account.clone(),
                token_program.clone(),
            ],
        )?;

        let owner_change_ix = spl_token::instruction::set_authority(
            token_program.key,
            bidder_ft_temp_account.key,
            Some(&pda),
            spl_token::instruction::AuthorityType::AccountOwner,
            bidder_account.key,
            &[], // owner_pubkey is default signer when the signer_pubkeys is empty.
        )?;
        msg!("Changing ownership of the token account...");
        invoke(
            &owner_change_ix,
            &[
                bidder_ft_temp_account.clone(),
                bidder_account.clone(),
                token_program.clone(),
            ],
        )?;

        if auction_info.highest_bidder_pubkey != Pubkey::default(){
            let transfer_to_previous_bidder_ix = spl_token::instruction::transfer(
                token_program.key,
                highest_bidder_ft_temp_account.key,
                highest_bidder_ft_returning_account.key,
                &pda,
                &[], // authority_pubkey is default signer when the signer_pubkeys is empty.
                auction_info.price,
            )?;
            msg!("Transferring FT to the previous highest bidder from the escrow account...");
            let signers_seeds: &[&[&[u8]]] = &[&[&b"escrow"[..], &[bump_seed]]];
            invoke_signed(
                &transfer_to_previous_bidder_ix,
                &[
                    highest_bidder_ft_temp_account.clone(),
                    highest_bidder_ft_returning_account.clone(),
                    pda_account.clone(),
                    token_program.clone(),
                ],
                signers_seeds,
            );

            Self::close_temporary_ft(
                token_program,
                highest_bidder_ft_temp_account,
                highest_bidder_account,
                pda,
                pda_account,
                signers_seeds,
            )?;
        }

        auction_info.price = price;
        auction_info.highest_bidder_pubkey = *bidder_account.key;
        auction_info.highest_bidder_ft_temp_pubkey = *bidder_ft_temp_account.key;
        auction_info.highest_bidder_ft_returning_pubkey = *bidder_ft_account.key;
        Auction::pack(auction_info, &mut escrow_account.try_borrow_mut_data()?)?;
        Ok(())
    }

    fn process_cancel(accounts: &[AccountInfo], program_id: &Pubkey) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let exhibitor_account = next_account_info(account_info_iter)?;

        if !exhibitor_account.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let exhibiting_nft_temp_account = next_account_info(account_info_iter)?;
        let exhibiting_nft_returning_account = next_account_info(account_info_iter)?;
        let escrow_account = next_account_info(account_info_iter)?;
        let auction_info = Auction::unpack(&escrow_account.try_borrow_data()?)?;

        if auction_info.exhibitor_pubkey != *exhibitor_account.key {
            return Err(ProgramError::InvalidAccountData);
        }
        if auction_info.exhibiting_nft_temp_pubkey != *exhibiting_nft_temp_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        if auction_info.highest_bidder_pubkey != Pubkey::default() {
            return Err(AuctionError::AlreadyBid.into());
        }

        let (pda, bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);
        let token_program = next_account_info(account_info_iter)?;
        let pda_account = next_account_info(account_info_iter)?;
        let signers_seeds: &[&[&[u8]]] = &[&[&b"escrow"[..], &[bump_seed]]];

        let exhibiting_nft_temp_account_data =
            TokenAccount::unpack(&exhibiting_nft_temp_account.try_borrow_data()?)?;
        let transfer_nft_to_exhibitor_ix = spl_token::instruction::transfer(
            token_program.key,
            exhibiting_nft_temp_account.key,
            exhibiting_nft_returning_account.key,
            &pda,
            &[], 
            exhibiting_nft_temp_account_data.amount,
        )?;
        msg!("Transferring NFT to the Exhibitor...");
        invoke_signed(
            &transfer_nft_to_exhibitor_ix,
            &[
                exhibiting_nft_temp_account.clone(),
                exhibiting_nft_returning_account.clone(),
                pda_account.clone(),
                token_program.clone(),
            ],
            signers_seeds,
        )?;

        Self::close_escrow(
            token_program,
            exhibiting_nft_temp_account,
            exhibitor_account,
            pda,
            pda_account,
            escrow_account,
            signers_seeds,
        )
    }

    fn process_close(accounts: &[AccountInfo], program_id: &Pubkey) -> ProgramResult {
        let account_info_iter = &mut accounts.iter();
        let highest_bidder_account = next_account_info(account_info_iter)?;

        if !highest_bidder_account.is_signer {
            return Err(ProgramError::MissingRequiredSignature);
        }

        let exhibitor_account = next_account_info(account_info_iter)?;
        let exhibiting_nft_temp_account = next_account_info(account_info_iter)?;
        let exhibitor_ft_receiving_account = next_account_info(account_info_iter)?;
        let highest_bidder_ft_temp_account = next_account_info(account_info_iter)?;
        let highest_bidder_nft_receiving_account = next_account_info(account_info_iter)?;
        let escrow_account = next_account_info(account_info_iter)?;
        let auction_info = Auction::unpack(&escrow_account.try_borrow_data()?)?;

        let sys_var_clock_account = next_account_info(account_info_iter)?;
        let clock = &Clock::from_account_info(sys_var_clock_account)?;

        if auction_info.end_at > clock.unix_timestamp {
            msg!(
                "Auction will end in {} seconds",
                (auction_info.end_at - clock.unix_timestamp)
            );
            return Err(AuctionError::ActiveAuction.into());
        }
        if auction_info.exhibitor_pubkey != *exhibitor_account.key {
            return Err(ProgramError::InvalidAccountData);
        }
        if auction_info.exhibiting_nft_temp_pubkey != *exhibiting_nft_temp_account.key {
            return Err(ProgramError::InvalidAccountData);
        }
        if auction_info.exhibitor_ft_receiving_pubkey != *exhibitor_ft_receiving_account.key {
            return Err(ProgramError::InvalidAccountData);
        }
        if auction_info.highest_bidder_ft_temp_pubkey != *highest_bidder_ft_temp_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        if auction_info.highest_bidder_pubkey != *highest_bidder_account.key {
            return Err(ProgramError::InvalidAccountData);
        }

        let (pda, bump_seed) = Pubkey::find_program_address(&[b"escrow"], program_id);
        let token_program = next_account_info(account_info_iter)?;
        let pda_account = next_account_info(account_info_iter)?;
        let signers_seeds: &[&[&[u8]]] = &[&[&b"escrow"[..], &[bump_seed]]];

        let exhibiting_nft_temp_account_data =
            TokenAccount::unpack(&exhibiting_nft_temp_account.try_borrow_data()?)?;

        let transfer_nft_to_highest_bidder_ix = spl_token::instruction::transfer(
            token_program.key,
            exhibiting_nft_temp_account.key,
            &highest_bidder_nft_receiving_account.key,
            &pda,
            &[], 
            exhibiting_nft_temp_account_data.amount,
        )?;
        msg!("Transferring NFT to the Highest Bidder...");
        invoke_signed(
            &transfer_nft_to_highest_bidder_ix,
            &[
                exhibiting_nft_temp_account.clone(),
                highest_bidder_nft_receiving_account.clone(),
                pda_account.clone(),
                token_program.clone(),
            ],
            signers_seeds,
        )?;

        let highest_bidder_ft_temp_account_data =
            TokenAccount::unpack(&highest_bidder_ft_temp_account.try_borrow_data()?)?;
        let transfer_ft_to_exhibitor_ix = spl_token::instruction::transfer(
            token_program.key,
            highest_bidder_ft_temp_account.key,
            &exhibitor_ft_receiving_account.key,
            &pda,
            &[], 
            highest_bidder_ft_temp_account_data.amount,
        )?;
        msg!("Transferring FT to the Exhibitor...");
        invoke_signed(
            &transfer_ft_to_exhibitor_ix,
            &[
                highest_bidder_ft_temp_account.clone(),
                exhibitor_ft_receiving_account.clone(),
                pda_account.clone(),
                token_program.clone(),
            ],
            signers_seeds,
        )?;

        Self::close_temporary_ft(
            token_program,
            highest_bidder_ft_temp_account,
            highest_bidder_account,
            pda,
            pda_account,
            signers_seeds,
        )?;

        Self::close_escrow(
            token_program,
            exhibiting_nft_temp_account,
            exhibitor_account,
            pda,
            pda_account,
            escrow_account,
            signers_seeds,
        )
    }

    fn close_escrow<'a, 'b>(
        token_program: &'a AccountInfo<'b>,
        exhibiting_nft_temp_account: &'a AccountInfo<'b>,
        exhibitor_account: &'a AccountInfo<'b>,
        pda: Pubkey,
        pda_account: &'a AccountInfo<'b>,
        escrow_account: &'a AccountInfo<'b>,
        signers_seed: &[&[&[u8]]],
    ) -> ProgramResult {
        let close_pdas_temp_acc_ix = spl_token::instruction::close_account(
            token_program.key,
            exhibiting_nft_temp_account.key,
            exhibitor_account.key,
            &pda,
            &[], // owner_pubkey is default signer when the signer_pubkeys is empty.
        )?;
        msg!("Closing the exhibitor's NFT temporary account...");
        invoke_signed(
            &close_pdas_temp_acc_ix,
            &[
                exhibiting_nft_temp_account.clone(),
                exhibitor_account.clone(),
                pda_account.clone(),
                token_program.clone(),
            ],
            signers_seed,
        );

        msg!("Closing the Escrow Account...");
        **exhibitor_account.try_borrow_mut_lamports()? = exhibitor_account
            .lamports()
            .checked_add(escrow_account.lamports())
            .ok_or(AuctionError::AmountOverflow)?;
        **escrow_account.try_borrow_mut_lamports()? = 0;
        *escrow_account.try_borrow_mut_data()? = &mut [];

        Ok(())
    }

    fn close_temporary_ft<'a, 'b>(
        token_program: &'a AccountInfo<'b>,
        highest_bidder_ft_temp_account: &'a AccountInfo<'b>,
        highest_bidder_account: &'a AccountInfo<'b>,
        pda: Pubkey,
        pda_account: &'a AccountInfo<'b>,
        signers_seeds: &[&[&[u8]]],
    ) -> ProgramResult {
        let close_highest_bidder_ft_temp_acc_ix = spl_token::instruction::close_account(
            token_program.key,
            highest_bidder_ft_temp_account.key,
            highest_bidder_account.key,
            &pda,
            &[],
        )?;
        msg!("Closing the Highest Bidder's FT temporary account...");
        invoke_signed(
            &close_highest_bidder_ft_temp_acc_ix,
            &[
                highest_bidder_ft_temp_account.clone(),
                highest_bidder_account.clone(),
                pda_account.clone(),
                token_program.clone(),
            ],
            signers_seeds,
        );

        Ok(())
    }
}