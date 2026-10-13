use flamekd::{Network, ReceivingAddress, RecvKey, SpendKey, TrackingAddress, ViewKey, HARDENED};

fn section<'a>(doc: &'a str, heading: &str) -> &'a str {
    doc.split_once(&format!("### {heading}\n"))
        .unwrap()
        .1
        .split("\n### ")
        .next()
        .unwrap()
}

fn check_vectors(formatted: &str, raw: &str, key: &SpendKey, network: Network) {
    let view = key.to_view();
    let recv = view.to_recv();
    let receiving = recv.to_address();
    let tracking = receiving.to_tracking_address();

    macro_rules! check {
        ($label:literal, $raw_label:literal, $value:expr, $ty:ty) => {
            let prefix = concat!($label, " = ");
            let expected = formatted
                .lines()
                .find_map(|line| line.strip_prefix(prefix))
                .expect(concat!("missing documented ", $label, " vector"));
            assert_eq!($value.to_bech32(network), expected);
            assert_eq!(<$ty>::from_bech32(expected, network).unwrap(), $value);
            assert_eq!(
                <$ty>::from_bech32(&expected.to_ascii_uppercase(), network).unwrap(),
                $value
            );
            let raw_hex: String = raw
                .split_once(concat!($raw_label, " ("))
                .expect(concat!("missing documented raw ", $label, " vector"))
                .1
                .split_once("```\n")
                .unwrap()
                .1
                .split_once("\n```")
                .unwrap()
                .0
                .split_whitespace()
                .collect();
            assert_eq!(hex::encode($value.to_bytes()), raw_hex);
        };
    }

    check!("spend", "Spend Key", key.clone(), SpendKey);
    check!("view", "View Key", view, ViewKey);
    check!("recv", "Recv Key", recv, RecvKey);
    check!(
        "receiving",
        "Receiving Address",
        receiving,
        ReceivingAddress
    );
    check!("tracking", "Tracking Address", tracking, TrackingAddress);
}

#[test]
fn documented_raw_and_formatted_vectors_match_key_derivation() {
    let address_doc = include_str!("../../docs/address.md");
    let key_doc = include_str!("../../docs/flamekd.md");
    let root = SpendKey::from_seed(&[0; 64]).unwrap();
    for (heading, path) in [
        ("m", &[][..]),
        ("m/0", &[0][..]),
        ("m/0'", &[HARDENED][..]),
        ("m/0'/16909060", &[HARDENED, 0x0102_0304][..]),
    ] {
        let key = path
            .iter()
            .try_fold(root.clone(), |key, &index| key.derive_child(index))
            .unwrap();
        check_vectors(
            section(address_doc, heading),
            section(key_doc, heading),
            &key,
            Network::Mainnet,
        );
    }
    check_vectors(
        section(address_doc, "Root with testnet HRPs"),
        section(key_doc, "m"),
        &root,
        Network::Testnet,
    );
}

#[test]
fn documented_actor_identifiers_round_trip() {
    use bech32::{primitives::decode::CheckedHrpstring, Bech32m, Hrp};

    let actor = section(
        include_str!("../../docs/address.md"),
        "Human-readable actor identifiers",
    );
    let documented = |name| {
        let prefix = format!("{name} = ");
        actor
            .lines()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap()
    };
    let raw = hex::decode(documented("actor_id")).unwrap();
    assert_eq!(raw, (0u8..32).collect::<Vec<_>>());
    for (network, hrp, length) in [("mainnet", "a", 60), ("testnet", "ta", 61)] {
        let encoded = documented(network);
        assert_eq!(encoded.len(), length);
        assert_eq!(
            bech32::encode::<Bech32m>(Hrp::parse(hrp).unwrap(), &raw).unwrap(),
            encoded
        );
        for text in [encoded.to_owned(), encoded.to_ascii_uppercase()] {
            let decoded = CheckedHrpstring::new::<Bech32m>(&text).unwrap();
            decoded.validate_segwit_padding().unwrap();
            assert!(decoded.hrp().lowercase_byte_iter().eq(hrp.bytes()));
            assert_eq!(decoded.byte_iter().collect::<Vec<_>>(), raw);
        }
    }
}
