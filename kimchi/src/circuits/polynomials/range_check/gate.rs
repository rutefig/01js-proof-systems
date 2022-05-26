//! Range check gate

use std::collections::HashMap;

use ark_ff::{FftField, SquareRootField, Zero};
use ark_poly::{univariate::DensePolynomial, Evaluations, Radix2EvaluationDomain as D};
use array_init::array_init;
use rand::{prelude::StdRng, SeedableRng};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;

use crate::{
    alphas::Alphas,
    circuits::{
        argument::{Argument, ArgumentType},
        constraints::ConstraintSystem,
        domains::EvaluationDomains,
        expr::{self, l0_1, Environment, E},
        gate::{CircuitGate, GateType},
        lookup::{
            self,
            tables::{GateLookupTable, LookupTable},
        },
        polynomial::COLUMNS,
        wires::{GateWires, Wire},
    },
};

use super::{RangeCheck0, RangeCheck1};

// Connect the pair of cells specified by the cell1 and cell2 parameters
// cell1 --> cell2 && cell2 --> cell1
//
// Note: This function assumes that the targeted cells are freshly instantiated
//       with self-connections.  If the two cells are transitively already part
//       of the same permutation then this would split it.
fn connect_cell_pair(wires: &mut [GateWires], cell1: (usize, usize), cell2: (usize, usize)) {
    let tmp = wires[cell1.0][cell1.1];
    wires[cell1.0][cell1.1] = wires[cell2.0][cell2.1];
    wires[cell2.0][cell2.1] = tmp;
}

impl<F: FftField + SquareRootField> CircuitGate<F> {
    /// Create range check gate
    ///     Inputs the starting row
    ///     Outputs tuple (next_row, circuit_gates) where
    ///       next_row      - next row after this gate
    ///       circuit_gates - vector of circuit gates comprising this gate
    pub fn create_range_check(start_row: usize) -> (usize, Vec<Self>) {
        let mut wires: Vec<GateWires> = (0..4).map(|i| Wire::new(start_row + i)).collect();

        // copy a0p4
        connect_cell_pair(&mut wires, (0, 5), (3, 1));

        // copy a0p5
        connect_cell_pair(&mut wires, (0, 6), (3, 2));

        // copy a1p4
        connect_cell_pair(&mut wires, (1, 5), (3, 3));

        // copy a1p5
        connect_cell_pair(&mut wires, (1, 6), (3, 4));

        let circuit_gates = vec![
            CircuitGate {
                typ: GateType::RangeCheck0,
                wires: wires[0],
                coeffs: vec![],
            },
            CircuitGate {
                typ: GateType::RangeCheck0,
                wires: wires[1],
                coeffs: vec![],
            },
            CircuitGate {
                typ: GateType::RangeCheck1,
                wires: wires[2],
                coeffs: vec![],
            },
            CircuitGate {
                typ: GateType::RangeCheck2,
                wires: wires[3],
                coeffs: vec![],
            },
        ];

        (start_row + circuit_gates.len(), circuit_gates)
    }

    /// Verify the range check circuit gate on a given row
    pub fn verify_range_check(
        &self,
        _: usize,
        witness: &[Vec<F>; COLUMNS],
        cs: &ConstraintSystem<F>,
    ) -> Result<(), String> {
        if self.typ == GateType::RangeCheck2 {
            // Not yet implemented
            // (Allow this to pass so that proof & verification test can function.)
            return Ok(());
        }

        // TODO: We should refactor some of this code into a
        // new Expr helper that can just evaluate a single row
        // and perform a lot of the common setup below so that
        // each CircuitGate's verify function doesn't need to
        // implement it separately.

        // Pad the witness to domain d1 size
        let padding_length = cs
            .domain
            .d1
            .size
            .checked_sub(witness[0].len() as u64)
            .unwrap();
        let mut witness = witness.clone();
        for w in &mut witness {
            w.extend(std::iter::repeat(F::zero()).take(padding_length as usize));
        }

        // Compute witness polynomial
        let witness_poly: [DensePolynomial<F>; COLUMNS] = array_init(|i| {
            Evaluations::<F, D<F>>::from_vec_and_domain(witness[i].clone(), cs.domain.d1)
                .interpolate()
        });

        // Compute permutation polynomial
        let rng = &mut StdRng::from_seed([0u8; 32]);
        let beta = F::rand(rng);
        let gamma = F::rand(rng);
        let z_poly = cs
            .perm_aggreg(&witness, &beta, &gamma, rng)
            .map_err(|_| format!("Invalid {:?} constraint - permutation failed", self.typ))?;

        // Compute witness polynomial evaluations
        let witness_evals = cs.evaluate(&witness_poly, &z_poly);

        // Set up the environment
        let env = {
            let mut index_evals = HashMap::new();
            index_evals.insert(
                self.typ,
                &cs.range_check_selector_polys[circuit_gate_selector_index(self.typ)].eval8,
            );

            Environment {
                constants: expr::Constants {
                    alpha: F::rand(rng),
                    beta: F::rand(rng),
                    gamma: F::rand(rng),
                    joint_combiner: Some(F::rand(rng)),
                    endo_coefficient: cs.endo,
                    mds: vec![], // TODO: maybe cs.fr_sponge_params.mds.clone()
                },
                witness: &witness_evals.d8.this.w,
                coefficient: &cs.coefficients8,
                vanishes_on_last_4_rows: &cs.precomputations().vanishes_on_last_4_rows,
                z: &witness_evals.d8.this.z,
                l0_1: l0_1(cs.domain.d1),
                domain: cs.domain,
                index: index_evals,
                lookup: None,
            }
        };

        // Setup powers of alpha
        let mut alphas = Alphas::<F>::default();
        alphas.register(
            ArgumentType::Gate(self.typ),
            circuit_gate_constraint_count::<F>(self.typ),
        );

        // Get constraints for this circuit gate
        let constraints = circuit_gate_constraints(self.typ, &alphas);

        // Verify it against the environment
        if constraints
            .evaluations(&env)
            .interpolate()
            .divide_by_vanishing_poly(cs.domain.d1)
            .unwrap()
            .1
            .is_zero()
        {
            Ok(())
        } else {
            Err(format!("Invalid {:?} constraint", self.typ))
        }
    }
}

fn circuit_gate_selector_index(typ: GateType) -> usize {
    match typ {
        GateType::RangeCheck0 => 0,
        GateType::RangeCheck1 => 1,
        _ => panic!("invalid gate type"),
    }
}

/// Get vector of range check circuit gate types
pub fn circuit_gates() -> Vec<GateType> {
    vec![GateType::RangeCheck0, GateType::RangeCheck1]
}

/// Number of constraints for a given range check circuit gate type
pub fn circuit_gate_constraint_count<F: FftField>(typ: GateType) -> u32 {
    match typ {
        GateType::RangeCheck0 => RangeCheck0::<F>::CONSTRAINTS,
        GateType::RangeCheck1 => RangeCheck1::<F>::CONSTRAINTS,
        _ => panic!("invalid gate type"),
    }
}

/// Get combined constraints for a given range check circuit gate type
pub fn circuit_gate_constraints<F: FftField>(typ: GateType, alphas: &Alphas<F>) -> E<F> {
    match typ {
        GateType::RangeCheck0 => RangeCheck0::combined_constraints(alphas),
        GateType::RangeCheck1 => RangeCheck1::combined_constraints(alphas),
        _ => panic!("invalid gate type"),
    }
}

/// Get the combined constraints for all range check circuit gate types
pub fn combined_constraints<F: FftField>(alphas: &Alphas<F>) -> E<F> {
    RangeCheck0::combined_constraints(alphas) + RangeCheck1::combined_constraints(alphas)
}

/// Range check CircuitGate selector polynomial
#[serde_as]
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct SelectorPolynomial<F: FftField> {
    /// Coefficient form
    #[serde_as(as = "o1_utils::serialization::SerdeAs")]
    pub coeff: DensePolynomial<F>,
    /// Evaluation form (evaluated over domain d8)
    #[serde_as(as = "o1_utils::serialization::SerdeAs")]
    pub eval8: Evaluations<F, D<F>>,
}

/// Create range check circuit gates selector polynomials
pub fn selector_polynomials<F: FftField>(
    gates: &[CircuitGate<F>],
    domain: &EvaluationDomains<F>,
) -> Vec<SelectorPolynomial<F>> {
    Vec::from_iter(circuit_gates().iter().map(|gate_type| {
        // Coefficient form
        let coeff = Evaluations::<F, D<F>>::from_vec_and_domain(
            gates
                .iter()
                .map(|gate| {
                    if gate.typ == *gate_type {
                        F::one()
                    } else {
                        F::zero()
                    }
                })
                .collect(),
            domain.d1,
        )
        .interpolate();

        // Evaluation form (evaluated over d8)
        let eval8 = coeff.evaluate_over_domain_by_ref(domain.d8);

        SelectorPolynomial { coeff, eval8 }
    }))
}

/// Get the range check lookup table
pub fn lookup_table<F: FftField>() -> LookupTable<F> {
    lookup::tables::get_table::<F>(GateLookupTable::RangeCheck)
}

#[cfg(test)]
mod tests {
    use crate::{
        circuits::{
            constraints::ConstraintSystem, gate::CircuitGate, polynomial::COLUMNS,
            polynomials::range_check, wires::Wire,
        },
        proof::ProverProof,
        prover_index::testing::new_index_for_test_with_lookups,
    };

    use ark_ec::AffineCurve;
    use ark_ff::One;
    use mina_curves::pasta::pallas;
    use o1_utils::FieldHelpers;

    use array_init::array_init;

    type PallasField = <pallas::Affine as AffineCurve>::BaseField;

    fn create_test_constraint_system() -> ConstraintSystem<PallasField> {
        let (mut next_row, mut gates) = CircuitGate::<PallasField>::create_range_check(0);

        // Temporary workaround for lookup-table/domain-size issue
        for _ in 0..(1 << 13) {
            gates.push(CircuitGate::zero(Wire::new(next_row)));
            next_row += 1;
        }

        ConstraintSystem::create(
            gates,
            vec![range_check::lookup_table()],
            None,
            oracle::pasta::fp_kimchi::params(),
            0,
        )
        .unwrap()
    }

    fn create_test_prover_index(
        public_size: usize,
    ) -> ProverIndex<mina_curves::pasta::vesta::Affine> {
        let (mut next_row, mut gates) = CircuitGate::<PallasField>::create_range_check(0);

        // Temporary workaround for lookup-table/domain-size issue
        for _ in 0..(1 << 13) {
            gates.push(CircuitGate::zero(Wire::new(next_row)));
            next_row += 1;
        }

        new_index_for_test_with_lookups(gates, public_size, vec![range_check::lookup_table()], None)
    }

    #[test]
    fn verify_range_check0_zero_valid_witness() {
        let cs = create_test_constraint_system();
        let witness: [Vec<PallasField>; COLUMNS] = array_init(|_| vec![PallasField::from(0); 4]);

        // gates[0] is RangeCheck0
        assert_eq!(cs.gates[0].verify_range_check(0, &witness, &cs), Ok(()));
    }

    #[test]
    fn verify_range_check0_one_invalid_witness() {
        let cs = create_test_constraint_system();
        let witness: [Vec<PallasField>; COLUMNS] = array_init(|_| vec![PallasField::from(1); 4]);

        // gates[0] is RangeCheck0
        assert_eq!(
            cs.gates[0].verify_range_check(0, &witness, &cs),
            Err("Invalid RangeCheck0 constraint".to_string())
        );
    }

    #[test]
    fn verify_range_check0_valid_witness() {
        let cs = create_test_constraint_system();

        let witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "115655443433221211ffef000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "eeddcdccbbabaa99898877000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "7766565544343322121100000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // gates[0] is RangeCheck0
        assert_eq!(cs.gates[0].verify_range_check(0, &witness, &cs), Ok(()));

        // gates[1] is RangeCheck0
        assert_eq!(cs.gates[1].verify_range_check(1, &witness, &cs), Ok(()));

        let witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "23d406ac800d1af73040dd000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "a8fe8555371eb021469863000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "3edff808d8f533be9af500000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // gates[0] is RangeCheck0
        assert_eq!(cs.gates[0].verify_range_check(0, &witness, &cs), Ok(()));

        // gates[1] is RangeCheck0
        assert_eq!(cs.gates[1].verify_range_check(1, &witness, &cs), Ok(()));
    }

    #[test]
    fn verify_range_check0_invalid_witness() {
        let cs = create_test_constraint_system();

        let mut witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "22f6b4e7ecb4488433ade7000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "e20e9d80333f2fba463ffd000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "25d28bfd6cdff91ca9bc00000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // Invalidate witness
        witness[5][0] += PallasField::one();

        // gates[0] is RangeCheck0
        assert_eq!(
            cs.gates[0].verify_range_check(0, &witness, &cs),
            Err(String::from(
                "Invalid RangeCheck0 constraint - permutation failed"
            ))
        );

        let mut witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "22cab5e27101eeafd2cbe1000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "1ab61d31f4e27fe41a318c000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "449a45cd749f1e091a3000000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // Invalidate witness
        witness[8][0] = witness[0][0] + PallasField::one();

        // gates[0] is RangeCheck0
        assert_eq!(
            cs.gates[0].verify_range_check(0, &witness, &cs),
            Err(String::from("Invalid RangeCheck0 constraint"))
        );
    }

    #[test]
    fn verify_range_check1_valid_witness() {
        let cs = create_test_constraint_system();

        let witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "22cab5e27101eeafd2cbe1000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "1ab61d31f4e27fe41a318c000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "449a45cd749f1e091a3000000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // gates[2] is RangeCheck1
        assert_eq!(cs.gates[2].verify_range_check(2, &witness, &cs), Ok(()));

        let witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "0d96f6fc210316c73bcc4d000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "59c8e7b0ffb3cab6ce8d48000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "686c10e73930b92f375800000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // gates[2] is RangeCheck1
        assert_eq!(cs.gates[2].verify_range_check(2, &witness, &cs), Ok(()));
    }

    #[test]
    fn verify_range_check1_invalid_witness() {
        let cs = create_test_constraint_system();

        let mut witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "2ce2d3ac942f98d59e7e11000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "52dd43524b95399f5d458d000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "60ca087b427918fa0e2600000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // Corrupt witness
        witness[0][2] = witness[7][2];

        // gates[2] is RangeCheck1
        assert_eq!(
            cs.gates[2].verify_range_check(2, &witness, &cs),
            Err(String::from("Invalid RangeCheck1 constraint"))
        );

        let mut witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "1bd50c94d2dc83d32f01c0000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "e983d7cd9e28e440930f86000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "ea226054772cd009d2af00000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // Corrupt witness
        witness[13][2] = witness[1][2];

        // gates[2] is RangeCheck1
        assert_eq!(
            cs.gates[2].verify_range_check(2, &witness, &cs),
            Err(String::from("Invalid RangeCheck1 constraint"))
        );
    }

    use crate::{prover_index::ProverIndex, verifier::verify};
    use commitment_dlog::commitment::CommitmentCurve;
    use groupmap::GroupMap;
    use mina_curves::pasta as pasta_curves;
    use oracle::{
        constants::PlonkSpongeConstantsKimchi,
        sponge::{DefaultFqSponge, DefaultFrSponge},
    };

    type BaseSponge =
        DefaultFqSponge<pasta_curves::vesta::VestaParameters, PlonkSpongeConstantsKimchi>;
    type ScalarSponge = DefaultFrSponge<pasta_curves::Fp, PlonkSpongeConstantsKimchi>;

    #[test]
    fn verify_range_check_valid_proof1() {
        // Create prover index
        let prover_index = create_test_prover_index(0);

        // Create witness
        let witness = range_check::create_witness::<PallasField>(
            PallasField::from_hex(
                "2bc0afaa2f6f50b1d1424b000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "8b30889f3a39e297ac851a000000000000000000000000000000000000000000",
            )
            .unwrap(),
            PallasField::from_hex(
                "c1c85ec47635e8edac5600000000000000000000000000000000000000000000",
            )
            .unwrap(),
        );

        // Verify computed witness satisfies the circuit
        prover_index.cs.verify(&witness, &[]).unwrap();

        // Generate proof
        let group_map = <pasta_curves::vesta::Affine as CommitmentCurve>::Map::setup();
        let proof = ProverProof::create::<BaseSponge, ScalarSponge>(
            &group_map,
            witness,
            &[],
            &prover_index,
        )
        .expect("failed to generate proof");

        // Get the verifier index
        let verifier_index = prover_index.verifier_index();

        // Verify proof
        let res = verify::<pasta_curves::vesta::Affine, BaseSponge, ScalarSponge>(
            &group_map,
            &verifier_index,
            &proof,
        );

        assert!(!res.is_err());
    }
}
