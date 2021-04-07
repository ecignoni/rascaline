use std::collections::BTreeSet;

use indexmap::IndexSet;
use itertools::Itertools;

use crate::systems::System;
use super::{SamplesIndexes, Indexes, IndexesBuilder, IndexValue};

/// `StructureSpeciesSamples` is used to represents samples corresponding to
/// full structures, where each chemical species in the structure is represented
/// separately.
///
/// The base set of indexes contains `structure` and `species` the  gradient
/// indexes also contains the `atom` inside the structure with respect to which
/// the gradient is taken and the `spatial` (i.e. x/y/z) index.
pub struct StructureSpeciesSamples;

impl SamplesIndexes for StructureSpeciesSamples {
    fn names(&self) -> Vec<&str> {
        vec!["structure", "species"]
    }

    #[time_graph::instrument(name = "StructureSpeciesSamples::indexes")]
    fn indexes(&self, systems: &mut [Box<dyn System>]) -> Indexes {
        let mut indexes = IndexesBuilder::new(self.names());
        for (i_system, system) in systems.iter().enumerate() {
            for &species in system.species().iter().collect::<BTreeSet<_>>() {
                indexes.add(&[
                    IndexValue::from(i_system), IndexValue::from(species)
                ]);
            }
        }
        return indexes.finish();
    }

    #[time_graph::instrument(name = "StructureSpeciesSamples::gradients_for")]
    fn gradients_for(&self, systems: &mut [Box<dyn System>], samples: &Indexes) -> Option<Indexes> {
        assert_eq!(samples.names(), self.names());

        let mut gradients = IndexesBuilder::new(vec!["structure", "species", "atom", "spatial"]);
        for value in samples.iter() {
            let i_system = value[0];
            let alpha = value[1];

            let system = &systems[i_system.usize()];
            let species = system.species();
            for (i_atom, &species) in species.iter().enumerate() {
                // only atoms with the same species participate to the gradient
                if species == alpha.usize() {
                    gradients.add(&[i_system, alpha, IndexValue::from(i_atom), IndexValue::from(0)]);
                    gradients.add(&[i_system, alpha, IndexValue::from(i_atom), IndexValue::from(1)]);
                    gradients.add(&[i_system, alpha, IndexValue::from(i_atom), IndexValue::from(2)]);
                }
            }
        }

        return Some(gradients.finish());
    }
}

/// `AtomSpeciesSamples` is used to represents atom-centered environments, where
/// each atom in a structure is described with a feature vector based on other
/// atoms inside a sphere centered on the central atom. These environments
/// include chemical species information.
///
/// The base set of indexes contains `structure`, `center` (i.e. central atom
/// index inside the structure), `species_center` and `species_neighbor`; the
/// gradient indexes also contains the `neighbor` inside the spherical cutoff
/// with respect to which the gradient is taken and the `spatial` (i.e x/y/z)
/// index.
pub struct AtomSpeciesSamples {
    /// spherical cutoff radius used to construct the atom-centered environments
    cutoff: f64,
    /// Is the central atom considered to be its own neighbor?
    self_contribution: bool,
}

impl AtomSpeciesSamples {
    /// Create a new `AtomSpeciesSamples` with the given `cutoff`, excluding
    /// self contributions.
    pub fn new(cutoff: f64) -> AtomSpeciesSamples {
        assert!(cutoff > 0.0 && cutoff.is_finite(), "cutoff must be positive for AtomSpeciesSamples");
        AtomSpeciesSamples {
            cutoff: cutoff,
            self_contribution: false,
        }
    }

    /// Create a new `AtomSpeciesSamples` with the given `cutoff`, including
    /// self contributions.
    pub fn with_self_contribution(cutoff: f64) -> AtomSpeciesSamples {
        assert!(cutoff > 0.0 && cutoff.is_finite(), "cutoff must be positive for AtomSpeciesSamples");
        AtomSpeciesSamples {
            cutoff: cutoff,
            self_contribution: true,
        }
    }
}

impl SamplesIndexes for AtomSpeciesSamples {
    fn names(&self) -> Vec<&str> {
        vec!["structure", "center", "species_center", "species_neighbor"]
    }

    #[time_graph::instrument(name = "AtomSpeciesSamples::indexes")]
    fn indexes(&self, systems: &mut [Box<dyn System>]) -> Indexes {
        // Accumulate indexes in a set first to ensure uniqueness of the indexes
        // even if their are multiple neighbors of the same specie around a
        // given center
        let mut set = BTreeSet::new();
        for (i_system, system) in systems.iter_mut().enumerate() {
            system.compute_neighbors(self.cutoff);
            let species = system.species();
            for pair in system.pairs() {
                let species_first = species[pair.first];
                let species_second = species[pair.second];

                set.insert((i_system, pair.first, species_first, species_second));
                set.insert((i_system, pair.second, species_second, species_first));
            };

            if self.self_contribution {
                for (center, &species) in species.iter().enumerate() {
                    set.insert((i_system, center, species, species));
                }
            }
        }

        let mut indexes = IndexesBuilder::new(self.names());
        for (s, c, a, b) in set {
            indexes.add(&[
                IndexValue::from(s), IndexValue::from(c), IndexValue::from(a), IndexValue::from(b)
            ]);
        }
        return indexes.finish();
    }

    #[time_graph::instrument(name = "AtomSpeciesSamples::gradients_for")]
    fn gradients_for(&self, systems: &mut [Box<dyn System>], samples: &Indexes) -> Option<Indexes> {
        assert_eq!(samples.names(), self.names());

        // We need IndexSet to yield the indexes in the right order, i.e. the
        // order corresponding to whatever was passed in `samples`
        let mut indexes = IndexSet::new();
        for requested in samples {
            let i_system = requested[0];
            let center = requested[1].usize();
            let species_neighbor = requested[3].usize();

            let system = &mut *systems[i_system.usize()];
            system.compute_neighbors(self.cutoff);

            let species = system.species();
            for pair in system.pairs_containing(center) {
                let species_first = species[pair.first];
                let species_second = species[pair.second];

                if pair.first == center && species_second == species_neighbor {
                    indexes.insert((i_system, pair.first, species_first, species_second, pair.second));
                } else if pair.second == center && species_first == species_neighbor {
                    indexes.insert((i_system, pair.second, species_second, species_first, pair.first));
                }
            }
        }

        let mut gradients = IndexesBuilder::new(vec![
            "structure", "center", "species_center", "species_neighbor",
            "neighbor", "spatial"
        ]);
        for (system, c, a, b, n) in indexes {
            let center = IndexValue::from(c);
            let alpha = IndexValue::from(a);
            let beta = IndexValue::from(b);
            let neighbor = IndexValue::from(n);
            gradients.add(&[system, center, alpha, beta, neighbor, IndexValue::from(0)]);
            gradients.add(&[system, center, alpha, beta, neighbor, IndexValue::from(1)]);
            gradients.add(&[system, center, alpha, beta, neighbor, IndexValue::from(2)]);
        }

        return Some(gradients.finish());
    }
}

/// `ThreeBodiesSpeciesSamples` is used to represents atom-centered environments
/// representing three body atomic density correlation; where the three bodies
/// include the central atom and two neighbors. These environments include
/// chemical species information.
///
/// The base set of indexes contains `structure`, `center` (i.e. central atom
/// index inside the structure), `species_center`, `species_neighbor_1` and
/// `species_neighbor2`; the gradient indexes also contains the `neighbor`
/// inside the spherical cutoff with respect to which the gradient is taken and
/// the `spatial` (i.e x/y/z) index.
pub struct ThreeBodiesSpeciesSamples {
    /// spherical cutoff radius used to construct the atom-centered environments
    cutoff: f64,
    /// Is the central atom considered to be its own neighbor?
    self_contribution: bool,
}

impl ThreeBodiesSpeciesSamples {
    /// Create a new `ThreeBodiesSpeciesSamples` with the given `cutoff`, excluding
    /// self contributions.
    pub fn new(cutoff: f64) -> ThreeBodiesSpeciesSamples {
        assert!(cutoff > 0.0 && cutoff.is_finite(), "cutoff must be positive for ThreeBodiesSpeciesSamples");
        ThreeBodiesSpeciesSamples {
            cutoff: cutoff,
            self_contribution: false,
        }
    }

    /// Create a new `ThreeBodiesSpeciesSamples` with the given `cutoff`,
    /// including self contributions.
    pub fn with_self_contribution(cutoff: f64) -> ThreeBodiesSpeciesSamples {
        assert!(cutoff > 0.0 && cutoff.is_finite(), "cutoff must be positive for ThreeBodiesSpeciesSamples");
        ThreeBodiesSpeciesSamples {
            cutoff: cutoff,
            self_contribution: true,
        }
    }
}

/// A Set built as a sorted vector
struct SortedVecSet<T> {
    data: Vec<T>
}

impl<T: Ord> SortedVecSet<T> {
    fn new() -> Self {
        SortedVecSet {
            data: Vec::new()
        }
    }

    fn insert(&mut self, value: T) {
        match self.data.binary_search(&value) {
            Ok(_) => {},
            Err(index) => self.data.insert(index, value),
        }
    }
}

impl<T> IntoIterator for SortedVecSet<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        self.data.into_iter()
    }
}

impl SamplesIndexes for ThreeBodiesSpeciesSamples {
    fn names(&self) -> Vec<&str> {
        vec!["structure", "center", "species_center", "species_neighbor_1", "species_neighbor_2"]
    }

    #[time_graph::instrument(name = "ThreeBodiesSpeciesSamples::indexes")]
    fn indexes(&self, systems: &mut [Box<dyn System>]) -> Indexes {
        // Accumulate indexes in a set first to ensure uniqueness of the indexes
        // even if their are multiple neighbors of the same specie around a
        // given center
        let mut set = SortedVecSet::new();

        let sort_pair = |i, j| {
            if i < j { (i, j) } else { (j, i) }
        };
        for (i_system, system) in systems.iter_mut().enumerate() {
            system.compute_neighbors(self.cutoff);
            let species = system.species();

            for center in 0..system.size() {
                for (i, j) in triplets_around(&**system, center) {
                    let (species_1, species_2) = sort_pair(species[i], species[j]);
                    set.insert((i_system, center, species[center], species_1, species_2));
                }
            }

            if self.self_contribution {
                for (center, &species_center) in species.iter().enumerate() {
                    set.insert((i_system, center, species_center, species_center, species_center));

                    for pair in system.pairs_containing(center) {
                        let neighbor = if pair.first == center {
                            pair.second
                        } else {
                            pair.first
                        };

                        let (species_1, species_2) = sort_pair(species_center, species[neighbor]);
                        set.insert((i_system, center, species_center, species_1, species_2));
                    }
                }
            }
        }

        let mut indexes = IndexesBuilder::new(self.names());
        for (structure, center, species_center, species_1, species_2) in set {
            indexes.add(&[
                IndexValue::from(structure),
                IndexValue::from(center),
                IndexValue::from(species_center),
                IndexValue::from(species_1),
                IndexValue::from(species_2)
            ]);
        }
        return indexes.finish();
    }

    #[time_graph::instrument(name = "ThreeBodiesSpeciesSamples::gradients_for")]
    fn gradients_for(&self, systems: &mut [Box<dyn System>], samples: &Indexes) -> Option<Indexes> {
        assert_eq!(samples.names(), self.names());

        let sort_pair = |i, j| {
            if i < j { (i, j) } else { (j, i) }
        };

        // We need IndexSet to yield the indexes in the right order, i.e. the
        // order corresponding to whatever was passed in `samples`
        let mut indexes = IndexSet::new();
        for requested in samples {
            let i_system = requested[0];
            let center = requested[1].usize();

            let system = &mut *systems[i_system.usize()];
            system.compute_neighbors(self.cutoff);

            let species = system.species();
            for (i, j) in triplets_around(&*system, center) {
                let (species_1, species_2) = sort_pair(species[i], species[j]);
                indexes.insert((i_system, center, species[center], species_1, species_2, i));
                indexes.insert((i_system, center, species[center], species_1, species_2, j));
            }
        }

        let mut gradients = IndexesBuilder::new(vec![
            "structure", "center", "species_center", "species_neighbor_1",
            "species_neighbor_2", "neighbor", "spatial"
        ]);
        for (system, center, species_center, species_neighbor_1, species_neighbor_2, neighbor) in indexes {
            let center = IndexValue::from(center);
            let species_center = IndexValue::from(species_center);
            let species_neighbor_1 = IndexValue::from(species_neighbor_1);
            let species_neighbor_2 = IndexValue::from(species_neighbor_2);
            let neighbor = IndexValue::from(neighbor);
            for spatial in 0..3_usize {
                gradients.add(&[
                    system, center, species_center, species_neighbor_1,
                    species_neighbor_2, neighbor, IndexValue::from(spatial)
                ]);
            }
        }

        return Some(gradients.finish());
    }
}

/// Build the list of triplet i-center-j
fn triplets_around(system: &dyn System, center: usize) -> impl Iterator<Item=(usize, usize)> + '_ {
    let pairs = system.pairs_containing(center);

    return pairs.iter().cartesian_product(pairs).map(move |(first_pair, second_pair)| {
        let i = if first_pair.first == center {
            first_pair.second
        } else {
            first_pair.first
        };

        let j = if second_pair.first == center {
            second_pair.second
        } else {
            second_pair.first
        };

        return (i, j);
    });
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::test_systems;

    /// Convenience macro to create IndexValue
    macro_rules! v {
        ($value: expr) => {
            crate::descriptor::indexes::IndexValue::from($value)
        };
    }

    #[test]
    fn structure() {
        let mut systems = test_systems(&["methane", "methane", "water"]).boxed();
        let indexes = StructureSpeciesSamples.indexes(&mut systems);
        assert_eq!(indexes.count(), 6);
        assert_eq!(indexes.names(), &["structure", "species"]);
        assert_eq!(indexes.iter().collect::<Vec<_>>(), vec![
            &[v!(0), v!(1)], &[v!(0), v!(6)],
            &[v!(1), v!(1)], &[v!(1), v!(6)],
            &[v!(2), v!(1)], &[v!(2), v!(123456)],
        ]);
    }

    #[test]
    fn structure_gradient() {
        let mut systems = test_systems(&["CH", "water"]).boxed();
        let (_, gradients) = StructureSpeciesSamples.with_gradients(&mut systems);
        let gradients = gradients.unwrap();
        assert_eq!(gradients.count(), 15);
        assert_eq!(gradients.names(), &["structure", "species", "atom", "spatial"]);

        assert_eq!(gradients.iter().collect::<Vec<_>>(), vec![
            // H channel in CH
            &[v!(0), v!(1), v!(0), v!(0)], &[v!(0), v!(1), v!(0), v!(1)], &[v!(0), v!(1), v!(0), v!(2)],
            // C channel in CH
            &[v!(0), v!(6), v!(1), v!(0)], &[v!(0), v!(6), v!(1), v!(1)], &[v!(0), v!(6), v!(1), v!(2)],
            // H channel in water
            &[v!(1), v!(1), v!(1), v!(0)], &[v!(1), v!(1), v!(1), v!(1)], &[v!(1), v!(1), v!(1), v!(2)],
            &[v!(1), v!(1), v!(2), v!(0)], &[v!(1), v!(1), v!(2), v!(1)], &[v!(1), v!(1), v!(2), v!(2)],
            // O channel in water
            &[v!(1), v!(123456), v!(0), v!(0)], &[v!(1), v!(123456), v!(0), v!(1)], &[v!(1), v!(123456), v!(0), v!(2)],
        ]);
    }

    #[test]
    fn partial_structure_gradient() {
        let mut indexes = IndexesBuilder::new(vec!["structure", "species"]);
        indexes.add(&[v!(2), v!(1)]);
        indexes.add(&[v!(0), v!(6)]);

        let mut systems = test_systems(&["CH", "water", "CH"]).boxed();
        let gradients = StructureSpeciesSamples.gradients_for(&mut systems, &indexes.finish());
        let gradients = gradients.unwrap();
        assert_eq!(gradients.names(), &["structure", "species", "atom", "spatial"]);

        assert_eq!(gradients.iter().collect::<Vec<_>>(), vec![
            // H channel in CH #2
            &[v!(2), v!(1), v!(0), v!(0)],
            &[v!(2), v!(1), v!(0), v!(1)],
            &[v!(2), v!(1), v!(0), v!(2)],
            // C channel in CH #1
            &[v!(0), v!(6), v!(1), v!(0)],
            &[v!(0), v!(6), v!(1), v!(1)],
            &[v!(0), v!(6), v!(1), v!(2)],
        ]);
    }

    #[test]
    fn atoms() {
        let mut systems = test_systems(&["CH", "water"]).boxed();
        let strategy = AtomSpeciesSamples::new(2.0);
        let indexes = strategy.indexes(&mut systems);
        assert_eq!(indexes.count(), 7);
        assert_eq!(indexes.names(), &["structure", "center", "species_center", "species_neighbor"]);
        assert_eq!(indexes.iter().collect::<Vec<_>>(), vec![
            // H in CH
            &[v!(0), v!(0), v!(1), v!(6)],
            // C in CH
            &[v!(0), v!(1), v!(6), v!(1)],
            // O in water
            &[v!(1), v!(0), v!(123456), v!(1)],
            // first H in water
            &[v!(1), v!(1), v!(1), v!(1)],
            &[v!(1), v!(1), v!(1), v!(123456)],
            // second H in water
            &[v!(1), v!(2), v!(1), v!(1)],
            &[v!(1), v!(2), v!(1), v!(123456)],
        ]);
    }

    #[test]
    fn atoms_self_contribution() {
        let mut systems = test_systems(&["CH"]).boxed();
        let strategy = AtomSpeciesSamples::new(2.0);
        let indexes = strategy.indexes(&mut systems);
        assert_eq!(indexes.count(), 2);
        assert_eq!(indexes.names(), &["structure", "center", "species_center", "species_neighbor"]);
        assert_eq!(indexes.iter().collect::<Vec<_>>(), vec![
            // H in CH
            &[v!(0), v!(0), v!(1), v!(6)],
            // C in CH
            &[v!(0), v!(1), v!(6), v!(1)],
        ]);

        let strategy = AtomSpeciesSamples::with_self_contribution(2.0);
        let indexes = strategy.indexes(&mut systems);
        assert_eq!(indexes.count(), 4);
        assert_eq!(indexes.names(), &["structure", "center", "species_center", "species_neighbor"]);
        assert_eq!(indexes.iter().collect::<Vec<_>>(), vec![
            // H in CH
            &[v!(0), v!(0), v!(1), v!(1)],
            &[v!(0), v!(0), v!(1), v!(6)],
            // C in CH
            &[v!(0), v!(1), v!(6), v!(1)],
            &[v!(0), v!(1), v!(6), v!(6)],
        ]);

        // we get entries even without proper neighbors
        let strategy = AtomSpeciesSamples::with_self_contribution(1.0);
        let indexes = strategy.indexes(&mut systems);
        assert_eq!(indexes.count(), 2);
        assert_eq!(indexes.names(), &["structure", "center", "species_center", "species_neighbor"]);
        assert_eq!(indexes.iter().collect::<Vec<_>>(), vec![
            // H in CH
            &[v!(0), v!(0), v!(1), v!(1)],
            // C in CH
            &[v!(0), v!(1), v!(6), v!(6)],
        ]);
    }

    #[test]
    fn atoms_gradient() {
        let mut systems = test_systems(&["CH", "water"]).boxed();
        let strategy = AtomSpeciesSamples::new(2.0);
        let (_, gradients) = strategy.with_gradients(&mut systems);
        let gradients = gradients.unwrap();

        assert_eq!(gradients.count(), 24);
        assert_eq!(gradients.names(), &["structure", "center", "species_center", "species_neighbor", "neighbor", "spatial"]);
        assert_eq!(gradients.iter().collect::<Vec<_>>(), vec![
            // H-C channel in CH
            &[v!(0), v!(0), v!(1), v!(6), v!(1), v!(0)],
            &[v!(0), v!(0), v!(1), v!(6), v!(1), v!(1)],
            &[v!(0), v!(0), v!(1), v!(6), v!(1), v!(2)],
            // C-H channel in CH
            &[v!(0), v!(1), v!(6), v!(1), v!(0), v!(0)],
            &[v!(0), v!(1), v!(6), v!(1), v!(0), v!(1)],
            &[v!(0), v!(1), v!(6), v!(1), v!(0), v!(2)],
            // O-H channel in water
            &[v!(1), v!(0), v!(123456), v!(1), v!(1), v!(0)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(1), v!(1)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(1), v!(2)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(2), v!(0)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(2), v!(1)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(2), v!(2)],
            // H-H channel in water, 1st atom
            &[v!(1), v!(1), v!(1), v!(1), v!(2), v!(0)],
            &[v!(1), v!(1), v!(1), v!(1), v!(2), v!(1)],
            &[v!(1), v!(1), v!(1), v!(1), v!(2), v!(2)],
            // H-O channel in water, 1st atom
            &[v!(1), v!(1), v!(1), v!(123456), v!(0), v!(0)],
            &[v!(1), v!(1), v!(1), v!(123456), v!(0), v!(1)],
            &[v!(1), v!(1), v!(1), v!(123456), v!(0), v!(2)],
            // H-H channel in water, 2nd atom
            &[v!(1), v!(2), v!(1), v!(1), v!(1), v!(0)],
            &[v!(1), v!(2), v!(1), v!(1), v!(1), v!(1)],
            &[v!(1), v!(2), v!(1), v!(1), v!(1), v!(2)],
            // H-O channel in water, 2nd atom
            &[v!(1), v!(2), v!(1), v!(123456), v!(0), v!(0)],
            &[v!(1), v!(2), v!(1), v!(123456), v!(0), v!(1)],
            &[v!(1), v!(2), v!(1), v!(123456), v!(0), v!(2)],
        ]);
    }

    #[test]
    fn partial_atoms_gradient() {
        let mut indexes = IndexesBuilder::new(vec!["structure", "center", "species_center", "species_neighbor"]);
        indexes.add(&[v!(1), v!(0), v!(123456), v!(1)]);
        indexes.add(&[v!(0), v!(0), v!(1), v!(6)]);
        indexes.add(&[v!(1), v!(1), v!(1), v!(1)]);

        let mut systems = test_systems(&["CH", "water"]).boxed();
        let strategy = AtomSpeciesSamples::new(2.0);
        let gradients = strategy.gradients_for(&mut systems, &indexes.finish());
        let gradients = gradients.unwrap();

        assert_eq!(gradients.names(), &["structure", "center", "species_center", "species_neighbor", "neighbor", "spatial"]);
        assert_eq!(gradients.iter().collect::<Vec<_>>(), vec![
            // O-H channel in water
            &[v!(1), v!(0), v!(123456), v!(1), v!(1), v!(0)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(1), v!(1)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(1), v!(2)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(2), v!(0)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(2), v!(1)],
            &[v!(1), v!(0), v!(123456), v!(1), v!(2), v!(2)],
            // H-C channel in CH
            &[v!(0), v!(0), v!(1), v!(6), v!(1), v!(0)],
            &[v!(0), v!(0), v!(1), v!(6), v!(1), v!(1)],
            &[v!(0), v!(0), v!(1), v!(6), v!(1), v!(2)],
            // H-H channel in water, 1st atom
            &[v!(1), v!(1), v!(1), v!(1), v!(2), v!(0)],
            &[v!(1), v!(1), v!(1), v!(1), v!(2), v!(1)],
            &[v!(1), v!(1), v!(1), v!(1), v!(2), v!(2)],
        ]);
    }

    #[test]
    fn three_bodies() {
        let mut systems = test_systems(&["CH", "water"]).boxed();
        let strategy = ThreeBodiesSpeciesSamples::new(2.0);
        let indexes = strategy.indexes(&mut systems);
        assert_eq!(indexes.count(), 9);
        assert_eq!(indexes.names(), &["structure", "center", "species_center", "species_neighbor_1", "species_neighbor_2"]);
        assert_eq!(indexes.iter().collect::<Vec<_>>(), vec![
            // C-H-C in CH
            &[v!(0), v!(0), v!(1), v!(6), v!(6)],
            // H-C-H in CH
            &[v!(0), v!(1), v!(6), v!(1), v!(1)],
            // H-O-H in water
            &[v!(1), v!(0), v!(123456), v!(1), v!(1)],
            // first H in water
            // H-H-H
            &[v!(1), v!(1), v!(1), v!(1), v!(1)],
            // H-H-O / O-H-H
            &[v!(1), v!(1), v!(1), v!(1), v!(123456)],
            // O-H-O
            &[v!(1), v!(1), v!(1), v!(123456), v!(123456)],
            // second H in water
            // H-H-H
            &[v!(1), v!(2), v!(1), v!(1), v!(1)],
            // H-H-O / O-H-H
            &[v!(1), v!(2), v!(1), v!(1), v!(123456)],
            // O-H-O
            &[v!(1), v!(2), v!(1), v!(123456), v!(123456)],
        ]);
    }

    #[test]
    fn three_bodies_self_contribution() {
        let mut systems = test_systems(&["water"]).boxed();
        // Only include O-H neighbors
        let strategy = ThreeBodiesSpeciesSamples::with_self_contribution(1.2);
        let indexes = strategy.indexes(&mut systems);
        assert_eq!(indexes.count(), 9);
        assert_eq!(indexes.names(), &["structure", "center", "species_center", "species_neighbor_1", "species_neighbor_2"]);
        assert_eq!(indexes.iter().collect::<Vec<_>>(), vec![
            // H-O-H
            &[v!(0), v!(0), v!(123456), v!(1), v!(1)],
            &[v!(0), v!(0), v!(123456), v!(1), v!(123456)],
            // O-O-O
            &[v!(0), v!(0), v!(123456), v!(123456), v!(123456)],
            // first H in water
            // H-H-H
            &[v!(0), v!(1), v!(1), v!(1), v!(1)],
            // O-H-O
            &[v!(0), v!(1), v!(1), v!(1), v!(123456)],
            &[v!(0), v!(1), v!(1), v!(123456), v!(123456)],
            // second H in water
            // H-H-H
            &[v!(0), v!(2), v!(1), v!(1), v!(1)],
            // O-H-O
            &[v!(0), v!(2), v!(1), v!(1), v!(123456)],
            &[v!(0), v!(2), v!(1), v!(123456), v!(123456)],
        ]);
    }

    #[test]
    fn three_bodies_gradients() {
        let mut systems = test_systems(&["water"]).boxed();
        let strategy = ThreeBodiesSpeciesSamples::new(2.0);
        let (_, gradients) = strategy.with_gradients(&mut systems);
        let gradients = gradients.unwrap();

        assert_eq!(gradients.count(), 30);
        assert_eq!(gradients.names(), &["structure", "center", "species_center", "species_neighbor_1", "species_neighbor_2", "neighbor", "spatial"]);
        assert_eq!(gradients.iter().collect::<Vec<_>>(), vec![
            // H-O-H in water
            &[v!(0), v!(0), v!(123456), v!(1), v!(1), v!(1), v!(0)],
            &[v!(0), v!(0), v!(123456), v!(1), v!(1), v!(1), v!(1)],
            &[v!(0), v!(0), v!(123456), v!(1), v!(1), v!(1), v!(2)],
            &[v!(0), v!(0), v!(123456), v!(1), v!(1), v!(2), v!(0)],
            &[v!(0), v!(0), v!(123456), v!(1), v!(1), v!(2), v!(1)],
            &[v!(0), v!(0), v!(123456), v!(1), v!(1), v!(2), v!(2)],
            // O-H-O, 1rst H
            &[v!(0), v!(1), v!(1), v!(123456), v!(123456), v!(0), v!(0)],
            &[v!(0), v!(1), v!(1), v!(123456), v!(123456), v!(0), v!(1)],
            &[v!(0), v!(1), v!(1), v!(123456), v!(123456), v!(0), v!(2)],
            // H-H-O, 1rst H
            &[v!(0), v!(1), v!(1), v!(1), v!(123456), v!(0), v!(0)],
            &[v!(0), v!(1), v!(1), v!(1), v!(123456), v!(0), v!(1)],
            &[v!(0), v!(1), v!(1), v!(1), v!(123456), v!(0), v!(2)],
            &[v!(0), v!(1), v!(1), v!(1), v!(123456), v!(2), v!(0)],
            &[v!(0), v!(1), v!(1), v!(1), v!(123456), v!(2), v!(1)],
            &[v!(0), v!(1), v!(1), v!(1), v!(123456), v!(2), v!(2)],
            // H-H-H 1rst H
            &[v!(0), v!(1), v!(1), v!(1), v!(1), v!(2), v!(0)],
            &[v!(0), v!(1), v!(1), v!(1), v!(1), v!(2), v!(1)],
            &[v!(0), v!(1), v!(1), v!(1), v!(1), v!(2), v!(2)],
            // O-H-O, 2nd H
            &[v!(0), v!(2), v!(1), v!(123456), v!(123456), v!(0), v!(0)],
            &[v!(0), v!(2), v!(1), v!(123456), v!(123456), v!(0), v!(1)],
            &[v!(0), v!(2), v!(1), v!(123456), v!(123456), v!(0), v!(2)],
            // H-H-O, 2nd H
            &[v!(0), v!(2), v!(1), v!(1), v!(123456), v!(0), v!(0)],
            &[v!(0), v!(2), v!(1), v!(1), v!(123456), v!(0), v!(1)],
            &[v!(0), v!(2), v!(1), v!(1), v!(123456), v!(0), v!(2)],
            &[v!(0), v!(2), v!(1), v!(1), v!(123456), v!(1), v!(0)],
            &[v!(0), v!(2), v!(1), v!(1), v!(123456), v!(1), v!(1)],
            &[v!(0), v!(2), v!(1), v!(1), v!(123456), v!(1), v!(2)],
            // H-H-H 2nd H
            &[v!(0), v!(2), v!(1), v!(1), v!(1), v!(1), v!(0)],
            &[v!(0), v!(2), v!(1), v!(1), v!(1), v!(1), v!(1)],
            &[v!(0), v!(2), v!(1), v!(1), v!(1), v!(1), v!(2)]
        ]);
    }
}
