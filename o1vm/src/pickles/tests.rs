use super::super::interpreters::mips::witness::SCRATCH_SIZE;
use super::proof::{ProofInputs, WitnessColumns};
use super::prover::prove;
use crate::pickles::verifier::verify;
use crate::{
    interpreters::mips::{
        constraints as mips_constraints, interpreter, interpreter::InterpreterEnv, Instruction,
    },
    pickles::{MAXIMUM_DEGREE_CONSTRAINTS, TOTAL_NUMBER_OF_CONSTRAINTS},
};
use ark_ff::{One, Zero};
use interpreter::{ITypeInstruction, JTypeInstruction, RTypeInstruction};
use kimchi::circuits::gate::CurrOrNext;
use kimchi::circuits::{domains::EvaluationDomains, expr::Expr};
use kimchi_msm::columns::Column;
use kimchi_msm::expr::E;
use log::debug;
use mina_curves::pasta::Fp;
use mina_curves::pasta::Fq;
use mina_curves::pasta::Pallas;
use mina_curves::pasta::PallasParameters;
use mina_poseidon::constants::PlonkSpongeConstantsKimchi;
use mina_poseidon::sponge::{DefaultFqSponge, DefaultFrSponge};
use o1_utils::tests::make_test_rng;
use poly_commitment::SRS;
use strum::{EnumCount, IntoEnumIterator};
#[test]
fn test_regression_constraints_with_selectors() {
    let constraints = {
        let mut mips_con_env = mips_constraints::Env::<Fp>::default();
        let mut constraints = Instruction::iter()
            .flat_map(|instr_typ| instr_typ.into_iter())
            .fold(vec![], |mut acc, instr| {
                interpreter::interpret_instruction(&mut mips_con_env, instr);
                let selector = mips_con_env.get_selector();
                let constraints_with_selector: Vec<E<Fp>> = mips_con_env
                    .get_constraints()
                    .into_iter()
                    .map(|c| selector.clone() * c)
                    .collect();
                acc.extend(constraints_with_selector);
                mips_con_env.reset();
                acc
            });
        constraints.extend(mips_con_env.get_selector_constraints());
        constraints
    };

    assert_eq!(constraints.len(), TOTAL_NUMBER_OF_CONSTRAINTS);

    let max_degree = constraints.iter().map(|c| c.degree(1, 0)).max().unwrap();
    assert_eq!(max_degree, MAXIMUM_DEGREE_CONSTRAINTS);
}

#[test]
// Sanity check that we have as many selector as we have instructions
fn test_regression_selectors_for_instructions() {
    let mips_con_env = mips_constraints::Env::<Fp>::default();
    let constraints = mips_con_env.get_selector_constraints();
    assert_eq!(
        // We substract 1 as we have one boolean check per sel
        // and 1 constraint to check that one and only one
        // sel is activated
        constraints.len() - 1,
        // We could use N_MIPS_SEL_COLS, but sanity check in case this value is
        // changed.
        RTypeInstruction::COUNT + JTypeInstruction::COUNT + ITypeInstruction::COUNT
    );
    // All instructions are degree 1 or 2.
    constraints
        .iter()
        .for_each(|c| assert!(c.degree(1, 0) == 2 || c.degree(1, 0) == 1));
}

#[test]
fn test_small_circuit() {
    let domain = EvaluationDomains::<Fq>::create(8).unwrap();
    let srs = SRS::create(8);
    let proof_input = ProofInputs::<Pallas> {
        evaluations: WitnessColumns {
            scratch: std::array::from_fn(|_| {
                vec![
                    Fq::one(),
                    Fq::one(),
                    Fq::one(),
                    Fq::one(),
                    Fq::one(),
                    Fq::one(),
                    Fq::one(),
                    Fq::one(),
                ]
            }),
            instruction_counter: vec![
                Fq::one(),
                Fq::one(),
                Fq::one(),
                Fq::one(),
                Fq::one(),
                Fq::one(),
                Fq::one(),
                Fq::one(),
            ],
            error: vec![
                -Fq::from((SCRATCH_SIZE + 1) as u64),
                -Fq::from((SCRATCH_SIZE + 1) as u64),
                -Fq::from((SCRATCH_SIZE + 1) as u64),
                -Fq::from((SCRATCH_SIZE + 1) as u64),
                -Fq::from((SCRATCH_SIZE + 1) as u64),
                -Fq::from((SCRATCH_SIZE + 1) as u64),
                -Fq::from((SCRATCH_SIZE + 1) as u64),
                -Fq::from((SCRATCH_SIZE + 1) as u64),
            ],
            selector: vec![
                Fq::zero(),
                Fq::zero(),
                Fq::zero(),
                Fq::zero(),
                Fq::zero(),
                Fq::zero(),
                Fq::zero(),
                Fq::zero(),
            ],
        },
    };
    let mut expr = Expr::literal(Fq::zero());
    for i in 0..SCRATCH_SIZE + 2 {
        expr += Expr::cell(Column::Relation(i), CurrOrNext::Curr);
    }
    expr *= Expr::cell(Column::DynamicSelector(0), CurrOrNext::Curr);
    let mut rng = make_test_rng(None);
    type BaseSponge = DefaultFqSponge<PallasParameters, PlonkSpongeConstantsKimchi>;
    type ScalarSponge = DefaultFrSponge<Fq, PlonkSpongeConstantsKimchi>;

    let proof = prove::<Pallas, BaseSponge, ScalarSponge, _>(
        domain,
        &srs,
        proof_input,
        &[expr.clone()],
        &mut rng,
    )
    .unwrap();
    let verif =
        verify::<Pallas, BaseSponge, ScalarSponge>(domain, &srs, &vec![expr.clone()], &proof);
    assert!(verif, "fdsf");
}
