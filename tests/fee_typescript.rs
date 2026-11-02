use lattice_sdk::wire::{Decimal, SafeInteger};
use lattice_sdk::{FeeWeight, ProjectedBlock, fee_to_outbid, transaction_weight};
use serde::Deserialize;
#[derive(Deserialize)]
struct WeightCase {
    size: SafeInteger,
    growth: SafeInteger,
    weight: Option<FeeWeight>,
    result: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RateCase {
    lowest_fee: Decimal,
    lowest_fee_weight: SafeInteger,
    weight: SafeInteger,
    result: String,
}
#[derive(Deserialize)]
struct Corpus {
    cases: Vec<WeightCase>,
    rates: Vec<RateCase>,
}
#[test]
fn actual_typescript_fee_calculations_match() {
    let corpus: Corpus = serde_json::from_str(include_str!("fee-typescript.json")).unwrap();
    assert_eq!(corpus.cases.len(), 192);
    assert_eq!(corpus.rates.len(), 48);
    for case in corpus.cases {
        assert_eq!(
            transaction_weight(case.size, case.growth, case.weight.as_ref()).to_string(),
            case.result
        );
    }
    for case in corpus.rates {
        let block = ProjectedBlock {
            transaction_count: SafeInteger(1),
            bytes: SafeInteger(1),
            lowest_fee: case.lowest_fee,
            lowest_fee_bytes: case.lowest_fee_weight,
            lowest_fee_weight: case.lowest_fee_weight,
            full: true,
        };
        assert_eq!(
            fee_to_outbid(&block, case.weight).unwrap().0.to_string(),
            case.result
        );
    }
}
