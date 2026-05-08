// One-shot discriminant probe + borsh-fixture printer for
// `RuntimeCall::Attestation(...)` variants. The TS SDK
// (`ligate-js/test/transaction.test.ts`) pins these exact bytes
// to catch wire-format drift between the chain (Rust borsh) and
// the SDK (hand-rolled TS borsh).
//
// Run with:
//
//     cargo run --example disc_probe -p ligate-bootstrap-cli
//
use attestation::{
    AttestorSet, AttestorSetId, AttestorSignature, CallMessage, PayloadHash, PubKey, Schema,
    SchemaId, MAX_ATTESTATION_SIGNATURES, MAX_ATTESTOR_SET_MEMBERS, MAX_ATTESTOR_SIGNATURE_BYTES,
};
use ligate_rollup::MockRollupSpec;
use ligate_stf::runtime::RuntimeCall;
use sov_modules_api::execution_mode::Native;
use sov_modules_api::{Address, SafeString, SafeVec, Spec};

type S = MockRollupSpec<Native>;
type SovAddress = <S as Spec>::Address;

fn print_call(label: &str, call: RuntimeCall<S>) {
    let bytes = borsh::to_vec(&call).unwrap();
    println!("== {label} ==");
    println!("  total: {} bytes", bytes.len());
    println!("  hex:   {}", hex::encode(&bytes));
    println!("  byte[0] (RuntimeCall variant): 0x{:02x}", bytes[0]);
    println!("  byte[1] (CallMessage variant): 0x{:02x}", bytes[1]);
}

fn main() {
    // RegisterAttestorSet — minimum: 1 member, threshold 1.
    let members =
        SafeVec::<PubKey, MAX_ATTESTOR_SET_MEMBERS>::try_from(vec![PubKey::from([0xAA; 32])])
            .unwrap();
    let register_set =
        RuntimeCall::Attestation(CallMessage::RegisterAttestorSet { members, threshold: 1 });
    print_call("RegisterAttestorSet (1-of-1, AAA pubkey)", register_set);

    // RegisterSchema — name = "x", v1, attestor_set_id = BB..BB,
    // no fee routing, payload_shape_hash all zero.
    let attestor_set: AttestorSetId = AttestorSetId::from([0xBB; 32]);
    let register_schema = RuntimeCall::Attestation(CallMessage::RegisterSchema {
        name: SafeString::try_from("x".to_string()).unwrap(),
        version: 1,
        attestor_set,
        fee_routing_bps: 0,
        fee_routing_addr: None,
        payload_shape_hash: [0u8; 32],
    });
    print_call("RegisterSchema (name='x', v1, BBB attestor_set)", register_schema);

    // SubmitAttestation — 1 signature.
    let schema_id = SchemaId::from([0xCC; 32]);
    let payload_hash = PayloadHash::from([0xDD; 32]);
    let sig = AttestorSignature {
        pubkey: PubKey::from([0xEE; 32]),
        sig: SafeVec::<u8, MAX_ATTESTOR_SIGNATURE_BYTES>::try_from(vec![0xFFu8; 64]).unwrap(),
    };
    let signatures =
        SafeVec::<AttestorSignature, MAX_ATTESTATION_SIGNATURES>::try_from(vec![sig]).unwrap();
    let submit = RuntimeCall::Attestation(CallMessage::SubmitAttestation {
        schema_id,
        payload_hash,
        signatures,
    });
    print_call("SubmitAttestation (1 sig, all-FF)", submit);

    // Deterministic id derivations the TS side can pin.
    println!();
    println!("== deterministic ids ==");
    let pks = vec![PubKey::from([0xAA; 32])];
    println!("  AttestorSet::derive_id(&[AAA], 1) = {}", AttestorSet::derive_id(&pks, 1));
    // SovAddress = MultiAddress<EthereumAddress>; we cast a 28-byte
    // fixture into the canonical chain-side Address.
    let owner: SovAddress = SovAddress::from(Address::from([0x42u8; 28]));
    println!("  Schema::derive_id(0x42x28, 'x', 1)  = {}", Schema::<S>::derive_id(&owner, "x", 1));
}
