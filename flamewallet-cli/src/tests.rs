use crate::amount::{format_qty, parse_qty, Flavor};
use crate::path::DerivationPath;
use crate::session::{coin_for_test, select, Coins};
use crate::store::{NetworkName, Secret, WalletFile};

const FLM: u64 = flamechain::SPARKS_PER_FLAME;

fn other() -> Flavor {
    Flavor([7; 32])
}

#[test]
fn flame_amounts_read_and_write_in_flames() {
    let flm = Flavor::flame();
    assert_eq!(parse_qty("1", flm).unwrap(), FLM);
    assert_eq!(parse_qty("1.5", flm).unwrap(), FLM + FLM / 2);
    assert_eq!(parse_qty(".00000001", flm).unwrap(), 1);
    assert_eq!(parse_qty("0.00000001", flm).unwrap(), 1);
    for bad in ["", ".", "0", "1.000000001", "-1", "1e3", "1,5", " 1"] {
        assert!(parse_qty(bad, flm).is_err(), "{bad:?}");
    }
    assert_eq!(format_qty(i128::from(FLM), flm), "1");
    assert_eq!(format_qty(150_000_000, flm), "1.5");
    assert_eq!(format_qty(1, flm), "0.00000001");
    assert_eq!(format_qty(-150_000_000, flm), "-1.5");
    assert_eq!(format_qty(0, flm), "0");
}

#[test]
fn other_flavors_are_whole_units() {
    assert_eq!(parse_qty("1500", other()).unwrap(), 1500);
    assert!(parse_qty("1.5", other()).is_err());
    assert_eq!(format_qty(1500, other()), "1500");
}

#[test]
fn a_flavor_reads_as_flm_or_hex() {
    assert_eq!("FLM".parse::<Flavor>().unwrap(), Flavor::flame());
    assert_eq!("flm".parse::<Flavor>().unwrap(), Flavor::flame());
    let hex = "07".repeat(32);
    assert_eq!(hex.parse::<Flavor>().unwrap(), other());
    assert!("07".parse::<Flavor>().is_err());
    // Scalar::ONE, little-endian, as Safely spells it too.
    assert_eq!(Flavor::flame().to_hex(), format!("01{}", "00".repeat(31)));
    assert_eq!(Flavor::flame().to_string(), "FLM");
    assert_eq!(other().to_string(), hex);
}

#[test]
fn selection_takes_the_largest_coins_of_one_flavor() {
    let flm = Flavor::flame();
    let coins = [
        coin_for_test(1, 3, flm, true),
        coin_for_test(2, 10, flm, true),
        coin_for_test(3, 5, flm, true),
        coin_for_test(4, 100, other(), true),
        // Pending: no proof, never chosen.
        coin_for_test(5, 1000, flm, false),
    ];
    let spendable: Vec<_> = coins.iter().filter(|c| c.is_spendable()).collect();

    let (chosen, total) = select(&spendable, flm, 12).unwrap();
    assert_eq!(chosen.iter().map(|c| c.id.0[0]).collect::<Vec<_>>(), [2, 3]);
    assert_eq!(total, 15);

    let (chosen, total) = select(&spendable, other(), 100).unwrap();
    assert_eq!(chosen.len(), 1);
    assert_eq!(total, 100);

    let error = select(&spendable, flm, 19).unwrap_err().to_string();
    assert!(error.contains("0.00000018"), "{error}");
}

#[test]
fn balances_are_per_flavor_flame_first_and_split_pending() {
    let flm = Flavor::flame();
    let coins = Coins {
        tip_height: 3,
        coins: vec![
            coin_for_test(1, 100, other(), true),
            coin_for_test(2, 7, flm, true),
            coin_for_test(3, 5, flm, false),
        ],
    };
    let balances = coins.balances();
    assert_eq!(balances.len(), 2);
    assert_eq!(balances[0].flavor, flm);
    assert_eq!((balances[0].confirmed, balances[0].pending), (7, 5));
    assert_eq!(balances[1].flavor, other());
    assert_eq!(balances[1].confirmed, 100);

    let empty = Coins {
        tip_height: 0,
        coins: Vec::new(),
    };
    assert_eq!(empty.balances()[0].flavor, flm);
}

#[test]
fn a_wallet_file_round_trips_and_is_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.json");
    let file = WalletFile {
        version: 1,
        network: NetworkName::Testnet,
        node: "http://127.0.0.1:8545".into(),
        secret: Secret::Seed {
            hex: "a1".repeat(64),
        },
        next_index: 1,
        watch: Vec::new(),
    };
    file.create(&path).unwrap();
    assert!(file.create(&path).is_err());

    let read = WalletFile::load(&path).unwrap();
    assert_eq!(read.next_index, 1);
    let wallet = read.open().unwrap();
    // The account the devnet's genesis allocation pays.
    let first = wallet
        .address(flamewallet_ffi::KeyPath {
            branch: flamewallet_ffi::RECEIVING,
            index: 0,
        })
        .unwrap();
    assert!(first.address.starts_with("t1"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn a_path_reads_as_branch_slash_index() {
    let path: DerivationPath = "2/0".parse().unwrap();
    assert_eq!((path.branch, path.index), (2, 0));
    assert_eq!(path.to_string(), "2/0");
    assert_eq!(
        "1/2147483647".parse::<DerivationPath>().unwrap().index,
        (1 << 31) - 1
    );
    for bad in [
        "",
        "2",
        "2/",
        "/0",
        "2/0/1",
        "-1/0",
        "+2/0",
        "2/2147483648",
        "a/0",
        " 2/0",
    ] {
        assert!(bad.parse::<DerivationPath>().is_err(), "{bad:?}");
    }
}

#[test]
fn watched_paths_round_trip_and_old_files_still_load() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wallet.json");
    let file = WalletFile {
        version: 1,
        network: NetworkName::Testnet,
        node: "http://127.0.0.1:8545".into(),
        secret: Secret::Seed {
            hex: "a1".repeat(64),
        },
        next_index: 1,
        watch: vec!["2/0".parse().unwrap(), "2/1".parse().unwrap()],
    };
    file.create(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(r#""2/0""#), "{text}");
    let read = WalletFile::load(&path).unwrap();
    assert_eq!(read.watch, file.watch);

    // A file written before paths were watched has no `watch` field.
    let old = dir.path().join("old.json");
    std::fs::write(
        &old,
        text.replace(",\n  \"watch\": [\n    \"2/0\",\n    \"2/1\"\n  ]", ""),
    )
    .unwrap();
    let old_text = std::fs::read_to_string(&old).unwrap();
    assert!(!old_text.contains("watch"), "{old_text}");
    assert!(WalletFile::load(&old).unwrap().watch.is_empty());
}
