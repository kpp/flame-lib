use std::{fmt, path::Path, time::Duration};

use flamekd::{Network, ReceivingAddress};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

pub const FEE_PER_BLOCK: i64 = 1_000;
pub const BLOCK_REWARD_SPARKS: i64 = 5_000_000_000;

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Conflict(String),
    Storage(rusqlite::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) | Self::Conflict(message) => f.write_str(message),
            Self::Storage(error) => write!(f, "Database error: {error}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContributionRequest {
    pub address: String,
    pub budget_sats: i64,
    pub duration: i64,
    pub request_id: String,
}

#[derive(Debug, Serialize)]
pub struct Submitted {
    pub id: i64,
    pub user_id: i64,
    pub start_block: i64,
}

#[derive(Debug, Serialize)]
pub struct Totals {
    pub contribution_count: i64,
    pub burned_sats: String,
    pub fees_sats: String,
    pub reward_sparks: String,
}

#[derive(Debug, Serialize)]
pub struct Contribution {
    pub id: i64,
    pub user_id: i64,
    pub address: String,
    pub budget_sats: i64,
    pub duration: i64,
    pub start_block: i64,
    pub burned_sats: String,
    pub fees_sats: String,
    pub reward_sparks: String,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub height: i64,
    pub totals: Totals,
    pub contributions: Vec<Contribution>,
    pub fee_per_block: i64,
    pub block_reward_sparks: i64,
}

#[derive(Debug, Serialize)]
pub struct RewardBalance {
    pub address: String,
    pub reward_sparks: String,
}

pub struct Database {
    connection: Connection,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self, Error> {
        let connection = Connection::open(path)?;
        // A second owner would run a second minting clock against the same ledger.
        connection.busy_timeout(Duration::ZERO)?;
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA locking_mode = EXCLUSIVE;
             BEGIN EXCLUSIVE;
             CREATE TABLE IF NOT EXISTS accounts (
                 id INTEGER PRIMARY KEY,
                 address TEXT NOT NULL UNIQUE,
                 reward_sparks INTEGER NOT NULL DEFAULT 0
                     CHECK(typeof(reward_sparks) = 'integer' AND reward_sparks >= 0)
             );
             CREATE TABLE IF NOT EXISTS contributions (
                 id INTEGER PRIMARY KEY,
                 request_id TEXT NOT NULL UNIQUE,
                 user_id INTEGER NOT NULL REFERENCES accounts(id),
                 budget_sats INTEGER NOT NULL CHECK(typeof(budget_sats) = 'integer'
                     AND budget_sats BETWEEN 2000 AND 100000000),
                 duration INTEGER NOT NULL CHECK(typeof(duration) = 'integer'
                     AND duration BETWEEN 100 AND 1000 AND duration % 50 = 0),
                 start_block INTEGER NOT NULL CHECK(typeof(start_block) = 'integer'
                     AND start_block >= 1),
                 CHECK(budget_sats >= duration * 1001)
             );
             CREATE TABLE IF NOT EXISTS blocks (
                 height INTEGER PRIMARY KEY CHECK(height >= 1),
                 burned_sats INTEGER NOT NULL CHECK(typeof(burned_sats) = 'integer'
                     AND burned_sats >= 0),
                 fees_sats INTEGER NOT NULL CHECK(typeof(fees_sats) = 'integer'
                     AND fees_sats >= 0),
                 reward_sparks INTEGER NOT NULL CHECK(typeof(reward_sparks) = 'integer'
                     AND ((burned_sats = 0 AND reward_sparks = 0)
                         OR (burned_sats > 0 AND reward_sparks = 5000000000)))
             );
             CREATE TABLE IF NOT EXISTS accruals (
                 height INTEGER NOT NULL REFERENCES blocks(height),
                 contribution_id INTEGER NOT NULL REFERENCES contributions(id),
                 burned_sats INTEGER NOT NULL CHECK(typeof(burned_sats) = 'integer'
                     AND burned_sats > 0),
                 fees_sats INTEGER NOT NULL CHECK(fees_sats = 1000),
                 reward_sparks INTEGER NOT NULL CHECK(typeof(reward_sparks) = 'integer'
                     AND reward_sparks >= 0),
                 PRIMARY KEY(height, contribution_id)
             );
             CREATE INDEX IF NOT EXISTS accruals_by_contribution
                 ON accruals(contribution_id);
             COMMIT;",
        )?;
        Ok(Self { connection })
    }

    pub fn submit(&mut self, mut request: ContributionRequest) -> Result<Submitted, Error> {
        request.address = ReceivingAddress::from_bech32(&request.address, Network::Testnet)
            .map_err(|_| {
                Error::Invalid("A valid testnet Flame receiving address is required.".into())
            })?
            .to_bech32(Network::Testnet);
        if !(2_000..=100_000_000).contains(&request.budget_sats)
            || !(100..=1_000).contains(&request.duration)
            || request.duration % 50 != 0
        {
            return Err(Error::Invalid(
                "Budget or duration is outside the supported range.".into(),
            ));
        }
        if request.budget_sats < request.duration * (FEE_PER_BLOCK + 1) {
            return Err(Error::Invalid(
                "Budget must cover 1,000 sat in fees and at least 1 sat burned per block.".into(),
            ));
        }
        if !(16..=64).contains(&request.request_id.len())
            || !request
                .request_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            return Err(Error::Invalid(
                "Submission ID must contain 16–64 ASCII letters, digits or hyphens.".into(),
            ));
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = transaction
            .query_row(
                "SELECT c.id, c.start_block, u.address, c.budget_sats, c.duration, c.user_id
             FROM contributions c JOIN accounts u ON u.id = c.user_id WHERE c.request_id = ?1",
                [&request.request_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .optional()?;
        if let Some((id, start_block, address, budget, duration, user_id)) = previous {
            return if address == request.address
                && budget == request.budget_sats
                && duration == request.duration
            {
                Ok(Submitted {
                    id,
                    user_id,
                    start_block,
                })
            } else {
                Err(Error::Conflict(
                    "Submission ID has already been used with different parameters.".into(),
                ))
            };
        }
        let start_block = transaction.query_row(
            "SELECT COALESCE(MAX(height), 0) + 1 FROM blocks",
            [],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT OR IGNORE INTO accounts(address) VALUES (?1)",
            [&request.address],
        )?;
        let user_id = transaction.query_row(
            "SELECT id FROM accounts WHERE address = ?1",
            [&request.address],
            |row| row.get(0),
        )?;
        transaction.execute(
            "INSERT INTO contributions(request_id, user_id, budget_sats, duration, start_block)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                request.request_id,
                user_id,
                request.budget_sats,
                request.duration,
                start_block
            ],
        )?;
        let id = transaction.last_insert_rowid();
        transaction.commit()?;
        Ok(Submitted {
            id,
            user_id,
            start_block,
        })
    }

    pub fn tick(&mut self) -> Result<(), Error> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let height: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(height), 0) + 1 FROM blocks",
            [],
            |row| row.get(0),
        )?;
        let mut active = {
            let mut statement = transaction.prepare(
                "SELECT id, user_id, budget_sats, duration, start_block FROM contributions
                 WHERE start_block <= ?1 AND start_block + duration > ?1 ORDER BY id",
            )?;
            let active = statement
                .query_map([height], |row| {
                    let budget: i64 = row.get(2)?;
                    let duration: i64 = row.get(3)?;
                    let offset = height - row.get::<_, i64>(4)?;
                    let net = budget - duration * FEE_PER_BLOCK;
                    Ok(Accrual {
                        id: row.get(0)?,
                        user_id: row.get(1)?,
                        burn: net / duration + i64::from(offset < net % duration),
                        reward: 0,
                        remainder: 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            active
        };
        let total_burn: i128 = active.iter().map(|item| i128::from(item.burn)).sum();
        let total_fees = (active.len() as i128) * i128::from(FEE_PER_BLOCK);
        let reward = if total_burn == 0 {
            0
        } else {
            BLOCK_REWARD_SPARKS
        };
        if reward > 0 {
            for item in &mut active {
                let numerator = i128::from(item.burn) * i128::from(reward);
                item.reward = (numerator / total_burn) as i64;
                item.remainder = numerator % total_burn;
            }
            let remaining = reward - active.iter().map(|item| item.reward).sum::<i64>();
            active.sort_unstable_by(|a, b| b.remainder.cmp(&a.remainder).then(a.id.cmp(&b.id)));
            for item in active.iter_mut().take(remaining as usize) {
                item.reward += 1;
            }
        }
        let amount = |value| {
            i64::try_from(value)
                .map_err(|_| Error::Invalid("Integer accounting capacity exceeded.".into()))
        };
        transaction.execute(
            "INSERT INTO blocks(height, burned_sats, fees_sats, reward_sparks) VALUES (?1, ?2, ?3, ?4)",
            params![height, amount(total_burn)?, amount(total_fees)?, reward],
        )?;
        for item in active {
            transaction.execute(
                "INSERT INTO accruals(height, contribution_id, burned_sats, fees_sats, reward_sparks)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![height, item.id, item.burn, FEE_PER_BLOCK, item.reward],
            )?;
            // SQLite may promote an overflowing addition to REAL; the account CHECK rejects it.
            transaction.execute(
                "UPDATE accounts SET reward_sparks = reward_sparks + ?1 WHERE id = ?2",
                params![item.reward, item.user_id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn snapshot(&self, full_history: bool) -> Result<Snapshot, Error> {
        let (height, totals) = self.connection.query_row(
            "SELECT COALESCE(MAX(height), 0), COALESCE(SUM(burned_sats), 0),
                 COALESCE(SUM(fees_sats), 0), COALESCE(SUM(reward_sparks), 0),
                 (SELECT COUNT(*) FROM contributions) FROM blocks",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    Totals {
                        contribution_count: row.get(4)?,
                        burned_sats: row.get::<_, i64>(1)?.to_string(),
                        fees_sats: row.get::<_, i64>(2)?.to_string(),
                        reward_sparks: row.get::<_, i64>(3)?.to_string(),
                    },
                ))
            },
        )?;
        let mut statement = self.connection.prepare(
            "SELECT c.id, u.address, c.budget_sats, c.duration, c.start_block,
                 COALESCE(SUM(a.burned_sats), 0), COALESCE(SUM(a.fees_sats), 0),
                 COALESCE(SUM(a.reward_sparks), 0), c.user_id
             FROM contributions c JOIN accounts u ON u.id = c.user_id
             LEFT JOIN accruals a ON a.contribution_id = c.id
             WHERE ?1 OR c.start_block + c.duration - 1 >= ?2
             GROUP BY c.id ORDER BY c.id DESC",
        )?;
        let contributions = statement
            .query_map(params![full_history, height - 10_000], |row| {
                Ok(Contribution {
                    id: row.get(0)?,
                    user_id: row.get(8)?,
                    address: row.get(1)?,
                    budget_sats: row.get(2)?,
                    duration: row.get(3)?,
                    start_block: row.get(4)?,
                    burned_sats: row.get::<_, i64>(5)?.to_string(),
                    fees_sats: row.get::<_, i64>(6)?.to_string(),
                    reward_sparks: row.get::<_, i64>(7)?.to_string(),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(Snapshot {
            height,
            totals,
            contributions,
            fee_per_block: FEE_PER_BLOCK,
            block_reward_sparks: BLOCK_REWARD_SPARKS,
        })
    }

    pub fn rewards(&self) -> Result<Vec<RewardBalance>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT address, reward_sparks FROM accounts ORDER BY address")?;
        let rewards = statement
            .query_map([], |row| {
                Ok(RewardBalance {
                    address: row.get(0)?,
                    reward_sparks: row.get::<_, i64>(1)?.to_string(),
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(rewards)
    }
}

struct Accrual {
    id: i64,
    user_id: i64,
    burn: i64,
    reward: i64,
    remainder: i128,
}

#[cfg(test)]
mod tests {
    use super::*;
    use flamekd::SpendKey;

    fn address(seed: u8) -> String {
        SpendKey::from_seed(&[seed; 64])
            .unwrap()
            .to_recv()
            .to_address()
            .to_bech32(Network::Testnet)
    }

    fn request(seed: u8, nonce: u8, net: i64) -> ContributionRequest {
        ContributionRequest {
            address: address(seed),
            budget_sats: 100_000 + net,
            duration: 100,
            request_id: format!("submission-{nonce:016}"),
        }
    }

    fn database() -> Database {
        Database::open(Path::new(":memory:")).unwrap()
    }

    #[test]
    fn validates_budget_duration_address_and_request_id() {
        let mut db = database();
        let valid = request(1, 1, 100);
        let mut invalid = valid.clone();
        invalid.budget_sats -= 1;
        assert!(matches!(db.submit(invalid), Err(Error::Invalid(_))));
        for duration in [0, 99, 101, 1_001] {
            let mut invalid = valid.clone();
            invalid.duration = duration;
            assert!(matches!(db.submit(invalid), Err(Error::Invalid(_))));
        }
        for budget in [1_999, 100_000_001, i64::MAX] {
            let mut invalid = valid.clone();
            invalid.budget_sats = budget;
            assert!(matches!(db.submit(invalid), Err(Error::Invalid(_))));
        }
        let mainnet = ReceivingAddress::from_bech32(&valid.address, Network::Testnet)
            .unwrap()
            .to_bech32(Network::Mainnet);
        let mut corrupt = valid.address.clone();
        let last = corrupt.pop().unwrap();
        corrupt.push(if last == 'q' { 'p' } else { 'q' });
        for address in ["not-an-address".into(), mainnet, corrupt] {
            let mut invalid = valid.clone();
            invalid.address = address;
            assert!(matches!(db.submit(invalid), Err(Error::Invalid(_))));
        }
        for request_id in ["short".into(), "a".repeat(65), "bad________nonce".into()] {
            let mut invalid = valid.clone();
            invalid.request_id = request_id;
            assert!(matches!(db.submit(invalid), Err(Error::Invalid(_))));
        }
        db.submit(valid).unwrap();
        assert_eq!(db.snapshot(false).unwrap().contributions.len(), 1);
        assert!(serde_json::from_str::<ContributionRequest>(
            r#"{"address":"a","budget_sats":2000,"duration":100,"request_id":"abcdefghijklmnop","seed":"secret"}"#
        ).is_err());
    }

    #[test]
    fn retries_are_idempotent_and_conflicting_reuse_is_rejected() {
        let mut db = database();
        let original = request(1, 1, 100);
        let first = db.submit(original.clone()).unwrap();
        db.tick().unwrap();
        let retry = db.submit(original.clone()).unwrap();
        assert_eq!((first.id, first.start_block), (retry.id, retry.start_block));
        let mut uppercase = original.clone();
        uppercase.address.make_ascii_uppercase();
        assert_eq!(db.submit(uppercase).unwrap().id, first.id);
        assert_eq!(db.rewards().unwrap().len(), 1);
        assert_eq!(db.rewards().unwrap()[0].address, original.address);
        let mut conflict = original.clone();
        conflict.budget_sats += 1;
        assert!(matches!(db.submit(conflict), Err(Error::Conflict(_))));
        let mut conflict = original;
        conflict.address = address(2);
        assert!(matches!(db.submit(conflict), Err(Error::Conflict(_))));
        assert_eq!(db.snapshot(false).unwrap().contributions.len(), 1);
    }

    #[test]
    fn user_ids_increment_per_canonical_address_not_per_contribution_or_retry() {
        let mut db = database();
        assert_eq!(db.snapshot(false).unwrap().height, 0);
        assert!(db.rewards().unwrap().is_empty());
        let original = request(1, 1, 100);
        let first = db.submit(original.clone()).unwrap();
        assert_eq!((first.id, first.user_id), (1, 1));
        assert_eq!(db.submit(original.clone()).unwrap().user_id, 1);
        let mut same_user = request(1, 2, 100);
        same_user.address.make_ascii_uppercase();
        let second = db.submit(same_user).unwrap();
        assert_eq!((second.id, second.user_id), (2, 1));
        let mut conflict = original;
        conflict.address = address(2);
        assert!(matches!(db.submit(conflict), Err(Error::Conflict(_))));
        let third = db.submit(request(2, 3, 100)).unwrap();
        assert_eq!((third.id, third.user_id), (3, 2));
        assert_eq!(db.submit(request(3, 4, 100)).unwrap().user_id, 3);
        let snapshot = db.snapshot(false).unwrap();
        let ids: Vec<_> = snapshot
            .contributions
            .iter()
            .rev()
            .map(|contribution| contribution.user_id)
            .collect();
        assert_eq!(ids, [1, 1, 2, 3]);
        for contribution in snapshot.contributions {
            assert_eq!(contribution.address, address(contribution.user_id as u8));
        }
        assert!(db
            .connection
            .execute("UPDATE contributions SET user_id = 999 WHERE id = 1", [])
            .is_err());
    }

    #[test]
    fn contribution_starts_next_block_and_spends_exact_budget() {
        let mut db = database();
        db.tick().unwrap();
        assert_eq!(db.snapshot(false).unwrap().totals.reward_sparks, "0");
        let submitted = db.submit(request(1, 1, 137)).unwrap();
        assert_eq!(submitted.start_block, 2);
        assert_eq!(
            db.snapshot(false).unwrap().contributions[0].burned_sats,
            "0"
        );
        for offset in 0..100 {
            db.tick().unwrap();
            let burn: i64 = db
                .connection
                .query_row(
                    "SELECT burned_sats FROM accruals WHERE height = ?1",
                    [offset + 2],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(burn, if offset < 37 { 2 } else { 1 });
        }
        let snapshot = db.snapshot(false).unwrap();
        assert_eq!(snapshot.totals.burned_sats, "137");
        assert_eq!(snapshot.totals.fees_sats, "100000");
        assert_eq!(
            snapshot.totals.reward_sparks,
            (100 * BLOCK_REWARD_SPARKS).to_string()
        );
        db.tick().unwrap();
        assert_eq!(
            db.snapshot(false).unwrap().totals.reward_sparks,
            snapshot.totals.reward_sparks
        );
    }

    #[test]
    fn dashboard_retains_10000_block_boundary_without_losing_lifetime_totals() {
        let mut db = database();
        db.submit(request(1, 1, 100)).unwrap();
        db.tick().unwrap();
        db.connection
            .execute(
                "INSERT INTO blocks(height, burned_sats, fees_sats, reward_sparks)
                 VALUES (10101, 0, 0, 0)",
                [],
            )
            .unwrap();
        let boundary = db.submit(request(2, 2, 100)).unwrap();
        db.connection
            .execute(
                "UPDATE contributions SET start_block = 2 WHERE id = ?1",
                [boundary.id],
            )
            .unwrap();
        let active = db.submit(request(3, 3, 100)).unwrap();
        db.connection
            .execute(
                "UPDATE contributions SET start_block = 10052 WHERE id = ?1",
                [active.id],
            )
            .unwrap();
        let future = db.submit(request(4, 4, 100)).unwrap();
        let recent = db.snapshot(false).unwrap();
        assert_eq!(recent.height, 10_101);
        assert_eq!(
            recent
                .contributions
                .iter()
                .map(|c| c.id)
                .collect::<Vec<_>>(),
            [future.id, active.id, boundary.id]
        );
        // The last funded block is inclusive: 100 is 10,001 blocks ago, 101 is 10,000.
        assert_eq!(recent.totals.contribution_count, 4);
        assert_eq!(recent.totals.burned_sats, "1");
        assert_eq!(recent.totals.fees_sats, "1000");
        assert_eq!(recent.totals.reward_sparks, "5000000000");
        let full = db.snapshot(true).unwrap();
        assert_eq!(full.contributions.len(), 4);
        assert_eq!(full.contributions[3].id, 1);
        assert_eq!(full.contributions[3].reward_sparks, "5000000000");
        assert_eq!(
            serde_json::to_value(&full.totals).unwrap(),
            serde_json::to_value(&recent.totals).unwrap()
        );
        assert_eq!(
            db.rewards()
                .unwrap()
                .iter()
                .find(|reward| reward.address == address(1))
                .unwrap()
                .reward_sparks,
            "5000000000"
        );
        let record_counts: (i64, i64, i64) = db
            .connection
            .query_row(
                "SELECT (SELECT COUNT(*) FROM contributions),
                        (SELECT COUNT(*) FROM blocks), (SELECT COUNT(*) FROM accruals)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(record_counts, (4, 2, 1));
    }

    #[test]
    fn proportional_rewards_conserve_every_spark_and_accumulate_by_address() {
        let mut db = database();
        for (seed, nonce, burn) in [(1, 1, 100), (2, 2, 100), (1, 3, 100)] {
            db.submit(request(seed, nonce, burn)).unwrap();
        }
        db.tick().unwrap();
        let snapshot = db.snapshot(false).unwrap();
        assert_eq!(snapshot.totals.burned_sats, "3");
        assert_eq!(snapshot.totals.fees_sats, "3000");
        assert_eq!(snapshot.totals.reward_sparks, "5000000000");
        let by_id: Vec<_> = snapshot
            .contributions
            .iter()
            .rev()
            .map(|c| c.reward_sparks.as_str())
            .collect();
        assert_eq!(by_id, ["1666666667", "1666666667", "1666666666"]);
        let rewards = db.rewards().unwrap();
        assert_eq!(
            rewards
                .iter()
                .find(|item| item.address == address(1))
                .unwrap()
                .reward_sparks,
            "3333333333"
        );
        assert_eq!(
            rewards
                .iter()
                .find(|item| item.address == address(2))
                .unwrap()
                .reward_sparks,
            "1666666667"
        );
        assert_eq!(snapshot.contributions[0].id, 3);
    }

    #[test]
    fn unequal_burn_weights_receive_proportional_rewards() {
        let mut db = database();
        db.submit(request(1, 1, 100)).unwrap();
        db.submit(request(2, 2, 200)).unwrap();
        db.tick().unwrap();
        let contributions = db.snapshot(false).unwrap().contributions;
        assert_eq!(contributions[0].reward_sparks, "3333333333");
        assert_eq!(contributions[1].reward_sparks, "1666666667");
    }

    #[test]
    fn restart_resumes_ledger_without_catching_up() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mint.sqlite3");
        {
            let mut db = Database::open(&path).unwrap();
            db.submit(request(1, 1, 100)).unwrap();
            db.tick().unwrap();
        }
        let mut db = Database::open(&path).unwrap();
        assert_eq!(db.snapshot(false).unwrap().height, 1);
        assert_eq!(db.snapshot(false).unwrap().contributions[0].user_id, 1);
        assert_eq!(db.rewards().unwrap()[0].reward_sparks, "5000000000");
        db.tick().unwrap();
        assert_eq!(db.snapshot(false).unwrap().height, 2);
        assert_eq!(db.rewards().unwrap()[0].reward_sparks, "10000000000");
        assert_eq!(db.submit(request(1, 2, 100)).unwrap().user_id, 1);
        assert_eq!(db.submit(request(2, 3, 100)).unwrap().user_id, 2);
    }

    #[test]
    fn database_file_has_one_owner_until_connection_closes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mint.sqlite3");
        let mut first = Database::open(&path).unwrap();
        first.tick().unwrap();
        assert!(matches!(Database::open(&path), Err(Error::Storage(_))));
        assert_eq!(first.snapshot(false).unwrap().height, 1);
        drop(first);
        let second = Database::open(&path).unwrap();
        assert_eq!(second.snapshot(false).unwrap().height, 1);
    }

    #[test]
    fn overflow_rolls_back_entire_tick() {
        let mut db = database();
        db.submit(request(1, 1, 100)).unwrap();
        db.submit(request(2, 2, 100)).unwrap();
        db.connection
            .execute(
                "UPDATE accounts SET reward_sparks = ?1 WHERE address = ?2",
                params![i64::MAX, address(2)],
            )
            .unwrap();
        assert!(matches!(db.tick(), Err(Error::Storage(_))));
        let snapshot = db.snapshot(false).unwrap();
        assert_eq!(snapshot.height, 0);
        assert_eq!(snapshot.totals.reward_sparks, "0");
        assert!(snapshot
            .contributions
            .iter()
            .all(|c| c.reward_sparks == "0"));
        assert_eq!(
            db.rewards()
                .unwrap()
                .iter()
                .find(|r| r.address == address(1))
                .unwrap()
                .reward_sparks,
            "0"
        );
        db.connection
            .execute(
                "UPDATE accounts SET reward_sparks = 0 WHERE address = ?1",
                [address(2)],
            )
            .unwrap();
        db.tick().unwrap();
        assert_eq!(db.snapshot(false).unwrap().height, 1);
    }
}
