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
    AttestationId, AttestorSet, AttestorSetId, AttestorSignature, CallMessage, PayloadHash, PubKey,
    Schema, SchemaId, MAX_ATTESTATION_SIGNATURES, MAX_ATTESTOR_SET_MEMBERS,
    MAX_ATTESTOR_SIGNATURE_BYTES,
};
use bounty::{
    AcceptancePredicate, AttestationClaim, BountyId, CallMessage as BountyCall,
    DisputeDecision as BountyDecision, DisputeGround as BountyGround, MAX_CLAIMS_PER_CALL,
};
use contract::{
    CallMessage as ContractCall, ContractId, DisputeDecision as ContractDecision,
    DisputeGround as ContractGround,
};
use ligate_rollup::MockRollupSpec;
use ligate_stf::runtime::RuntimeCall;
use sov_bank::Amount;
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

    // ---- Bounty module (RuntimeCall::Bounty = 0x0A) ----
    // PostBounty with the trivial `Any` acceptance predicate.
    let post_any = RuntimeCall::Bounty(BountyCall::PostBounty {
        board_schema_id: SchemaId::from([0xCC; 32]),
        pool: Amount::new(1_000_000_000),
        per_attestation: Amount::new(100_000_000),
        acceptance: AcceptancePredicate::Any,
        expiry_da_height: 12_345,
        dispute_window_blocks: 100,
    });
    print_call("Bounty::PostBounty (Any, CC schema)", post_any);

    // PostBounty with the `PeerCount` predicate (covers the disc 0x03 + u8 arm).
    let post_peer = RuntimeCall::Bounty(BountyCall::PostBounty {
        board_schema_id: SchemaId::from([0xCC; 32]),
        pool: Amount::new(1_000_000_000),
        per_attestation: Amount::new(100_000_000),
        acceptance: AcceptancePredicate::PeerCount { min_attestors: 3 },
        expiry_da_height: 12_345,
        dispute_window_blocks: 100,
    });
    print_call("Bounty::PostBounty (PeerCount=3)", post_peer);

    // ClaimBounty with one AttestationClaim.
    let claims =
        SafeVec::<AttestationClaim, MAX_CLAIMS_PER_CALL>::try_from(vec![AttestationClaim {
            attestation_id: AttestationId::from([0x22; 32]),
        }])
        .unwrap();
    let claim = RuntimeCall::Bounty(BountyCall::ClaimBounty {
        bounty_id: BountyId::from([0x11; 32]),
        claims,
    });
    print_call("Bounty::ClaimBounty (1 claim, 0x22 attestation)", claim);

    // DisputeAttestation (ground = Other = disc 0x03).
    let dispute = RuntimeCall::Bounty(BountyCall::DisputeAttestation {
        bounty_id: BountyId::from([0x11; 32]),
        attestation_id: AttestationId::from([0x22; 32]),
        ground: BountyGround::Other,
    });
    print_call("Bounty::DisputeAttestation (ground=Other)", dispute);

    // ResolveDispute (decision = Reject = disc 0x01).
    let resolve = RuntimeCall::Bounty(BountyCall::ResolveDispute {
        bounty_id: BountyId::from([0x11; 32]),
        attestation_id: AttestationId::from([0x22; 32]),
        decision: BountyDecision::Reject,
    });
    print_call("Bounty::ResolveDispute (decision=Reject)", resolve);

    // CancelBounty (single-id shape; FinaliseBounty is identical bar the disc).
    let cancel =
        RuntimeCall::Bounty(BountyCall::CancelBounty { bounty_id: BountyId::from([0x11; 32]) });
    print_call("Bounty::CancelBounty (0x11 bounty)", cancel);

    // ---- Contract module (RuntimeCall::Contracts = 0x0B) ----
    // PostContract: arbiter address + criteria hash + fee bps.
    let arbiter: SovAddress = SovAddress::from(Address::from([0x42u8; 28]));
    let post_contract = RuntimeCall::Contracts(ContractCall::PostContract {
        arbiter,
        criteria_doc_hash: [0x33; 32],
        pool: Amount::new(2_000_000_000),
        expiry_da_height: 999,
        dispute_window_blocks: 50,
        arbiter_fee_bps: 500,
    });
    print_call("Contracts::PostContract (0x42 arbiter, 0x33 criteria)", post_contract);

    // CommitToContract: contract id + commit hash + bond.
    let commit = RuntimeCall::Contracts(ContractCall::CommitToContract {
        contract_id: ContractId::from([0x44; 32]),
        commit_hash: [0x55; 32],
        bond: Amount::new(500_000_000),
    });
    print_call("Contracts::CommitToContract (0x44 contract, 0x55 commit)", commit);

    // DeliverContract: contract id + deliverable attestation id.
    let deliver = RuntimeCall::Contracts(ContractCall::DeliverContract {
        contract_id: ContractId::from([0x44; 32]),
        deliverable_attestation_id: AttestationId::from([0x22; 32]),
    });
    print_call("Contracts::DeliverContract (0x44 contract, 0x22 attestation)", deliver);

    // RejectDelivery (ground = Other = disc 0x03 in the contract enum).
    let reject = RuntimeCall::Contracts(ContractCall::RejectDelivery {
        contract_id: ContractId::from([0x44; 32]),
        ground: ContractGround::Other,
    });
    print_call("Contracts::RejectDelivery (ground=Other)", reject);

    // ResolveContractDispute (decision = RejectDelivery = disc 0x01).
    let resolve_c = RuntimeCall::Contracts(ContractCall::ResolveContractDispute {
        contract_id: ContractId::from([0x44; 32]),
        decision: ContractDecision::RejectDelivery,
    });
    print_call("Contracts::ResolveContractDispute (RejectDelivery)", resolve_c);

    // AcceptDelivery (single-id shape; CancelContract + FinalizeDelivery identical bar the disc).
    let accept = RuntimeCall::Contracts(ContractCall::AcceptDelivery {
        contract_id: ContractId::from([0x44; 32]),
    });
    print_call("Contracts::AcceptDelivery (0x44 contract)", accept);

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
