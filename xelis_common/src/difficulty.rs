use crate::config::{MINIMUM_DIFFICULTY, BLOCK_TIME_MILLIS};
use crate::crypto::hash::Hash;
use num_bigint::{BigUint, ToBigUint};
use thiserror::Error;
use num_traits::One;
use log::trace;

const E: f64 = 2.71828182845905;
const M: f64 = 8f64;

#[derive(Error, Debug)]
pub enum DifficultyError {
    #[error("Difficulty cannot be a value zero")]
    DifficultyCannotBeZero,
    #[error("Error while converting value to BigUint")]
    ErrorOnConversionBigUint
}


pub fn check_difficulty(hash: &Hash, difficulty: u64) -> Result<bool, DifficultyError> {
    let big_diff = difficulty_to_big(difficulty)?;
    let big_hash = hash_to_big(hash);

    Ok(big_hash <= big_diff)
}

pub fn difficulty_to_big(difficulty: u64) -> Result<BigUint, DifficultyError> {
    if difficulty == 0 {
        return Err(DifficultyError::DifficultyCannotBeZero)
    }

    let big_diff = match ToBigUint::to_biguint(&difficulty) {
        Some(v) => v,
        None => return Err(DifficultyError::ErrorOnConversionBigUint)
    };
    let one_lsh_256 = BigUint::one() << 256;
    Ok(one_lsh_256 / big_diff)
}

pub fn hash_to_big(hash: &Hash) -> BigUint {
    BigUint::from_bytes_be(hash.as_bytes())
}

pub fn calculate_difficulty(parent_timestamp: u128, new_timestamp: u128, previous_difficulty: u64) -> u64 {
    let mut solve_time: u128 = new_timestamp - parent_timestamp;
    if solve_time > (BLOCK_TIME_MILLIS as u128 * 2) {
        solve_time = BLOCK_TIME_MILLIS as u128 * 2;
    }

    let easypart = (E.powf((1f64 - solve_time as f64 / BLOCK_TIME_MILLIS as f64) / M) * 10000f64) as i64;
    let diff = ((previous_difficulty as i64 * easypart) / 10000) as u64;
    trace!("Difficulty calculated, easypart: {}, previous diff: {}, diff: {}", easypart, previous_difficulty, diff);

    if diff < MINIMUM_DIFFICULTY {
       return MINIMUM_DIFFICULTY
    }

    diff
}