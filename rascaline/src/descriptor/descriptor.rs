use std::collections::BTreeMap;
use indexmap::set::IndexSet;

use ndarray::{Array2, s};

use super::{Indexes, IndexesBuilder, IndexValue};

pub struct Descriptor {
    /// An array of environments.count() by features.count() values
    pub values: Array2<f64>,
    pub environments: Indexes,
    pub features: Indexes,
    /// Gradients of the descriptor with respect to one atomic position
    pub gradients: Option<Array2<f64>>,
    pub gradients_indexes: Option<Indexes>,
}

impl Default for Descriptor {
    fn default() -> Self { Self::new() }
}

impl Descriptor {
    pub fn new() -> Descriptor {
        let indexes = IndexesBuilder::new(vec![]).finish();
        return Descriptor {
            values: Array2::zeros((0, 0)),
            environments: indexes.clone(),
            features: indexes,
            gradients: None,
            gradients_indexes: None,
        }
    }

    pub(crate) fn prepare(
        &mut self,
        environments: Indexes,
        features: Indexes,
    ) {
        self.environments = environments;
        self.features = features;

        // resize the 'values' array if needed, and set the requested initial value
        let shape = (self.environments.count(), self.features.count());
        resize_and_reset(&mut self.values, shape);

        self.gradients = None;
        self.gradients_indexes = None;
    }

    pub(crate) fn prepare_gradients(
        &mut self,
        environments: Indexes,
        gradients: Indexes,
        features: Indexes,
    ) {
        // basic sanity check
        assert_eq!(gradients.names().last(), Some(&"spatial"), "the last index of gradient should be spatial");

        self.environments = environments;
        self.features = features;

        // resize the 'values' array if needed, and set the requested initial value
        let shape = (self.environments.count(), self.features.count());
        resize_and_reset(&mut self.values, shape);

        let gradient_shape = (gradients.count(), self.features.count());
        self.gradients_indexes = Some(gradients);

        if let Some(array) = &mut self.gradients {
            // resize the 'gradient' array if needed, and set the requested initial value
            resize_and_reset(array, gradient_shape);
        } else {
            // create a new gradient array
            let array = Array2::from_elem(gradient_shape, 0.0);
            self.gradients = Some(array);
        }
    }

    pub fn densify(&mut self, variable: &str) {
        let new_environments = remove_from_indexes(&self.environments, variable);

        let new_gradients = self.gradients_indexes.as_ref().map(|indexes| {
            let gradients = remove_from_indexes(indexes, variable);

            if gradients.new_features != new_environments.new_features {
                panic!("gradient indexes contains a different values for {} than the environment indexes", variable);
            }

            return gradients;
        });

        // new feature indexes, add `variable` in the front
        let mut feature_names = vec![variable];
        feature_names.extend(self.features.names());
        let mut new_features = IndexesBuilder::new(feature_names);
        for new in new_environments.new_features {
            for feature in self.features.iter() {
                let mut cleaned = vec![new];
                cleaned.extend(feature);
                new_features.add(&cleaned);
            }
        }
        let new_features = new_features.finish();
        let old_feature_size = self.features.count();

        // copy values as needed
        let mut new_values = Array2::zeros((new_environments.indexes.count(), new_features.count()));
        for (new, old) in new_environments.mapping {
            let value = self.values.slice(s![old, ..]);
            let start = new.feature_block * old_feature_size;
            let stop = (new.feature_block + 1) * old_feature_size;
            new_values.slice_mut(s![new.environment, start..stop]).assign(&value);
        }

        if let Some(self_gradients) = &self.gradients {
            let new_gradients = new_gradients.expect("missing densified gradients");

            let mut gradients = Array2::zeros((new_gradients.indexes.count(), new_features.count()));
            for (new, old) in new_gradients.mapping {
                let value = self_gradients.slice(s![old, ..]);
                let start = new.feature_block * old_feature_size;
                let stop = (new.feature_block + 1) * old_feature_size;
                gradients.slice_mut(s![new.environment, start..stop]).assign(&value);
            }

            self.gradients = Some(gradients);
            self.gradients_indexes = Some(new_gradients.indexes);
        }

        self.features = new_features;
        self.environments = new_environments.indexes;
        self.values = new_values;
    }
}

fn resize_and_reset(array: &mut Array2<f64>, shape: (usize, usize)) {
    // extract data by replacing array with a temporary value
    let mut tmp = Array2::zeros((0, 0));
    std::mem::swap(array, &mut tmp);

    let mut data = tmp.into_raw_vec();
    data.resize(shape.0 * shape.1, 0.0);

    let values = Array2::from_shape_vec(shape, data).expect("wrong array shape");
    let _ = std::mem::replace(array, values);
}

/// Representation of an environment/gradient index after densification
#[derive(Debug, Clone, Ord, PartialOrd, Eq, PartialEq)]
struct DensifiedIndex {
    /// Index of the new environment/gradient in the value/gradients array
    environment: usize,
    /// Index of the feature **block** corresponding to the moved variable
    feature_block: usize,
}

/// Results of moving a given variable from Indexes
struct RemovedResult {
    /// New Indexes, without the variable
    indexes: Indexes,
    /// Values taken by the variable in the original Index
    ///
    /// This needs to be a IndexSet to keep the same order as in the initial
    /// Indexes.
    new_features: IndexSet<IndexValue>,
    /// Mapping from the updated index to the original position in Indexes
    mapping: BTreeMap<DensifiedIndex, usize>,
}

/// Remove the given `variable` from the `indexes`, returning the updated
/// `indexes` and a set of all the values taken by the removed one.
fn remove_from_indexes(indexes: &Indexes, variable: &str) -> RemovedResult {
    let variable_i = match indexes.names().iter().position(|&name| name == variable) {
        Some(index) => index,
        None => panic!(
            "can not densify along '{}' which is not present in the environments: [{}]",
            variable, indexes.names().join(", ")
        )
    };

    let mut mapping = BTreeMap::new();

    // collect all different indexes in a set. Assuming we are densifying
    // along the first index, we want to convert [[2, 3, 0], [1, 3, 0]]
    // to [[3, 0]].
    let mut new_indexes = IndexSet::new();
    let mut new_features = IndexSet::new();
    for (old, index) in indexes.iter().enumerate() {
        new_features.insert(index[variable_i]);

        let mut cleaned = index[0..variable_i].to_vec();
        cleaned.extend(&index[(variable_i + 1)..]);
        new_indexes.insert(cleaned);

        let densified = DensifiedIndex{
            environment: new_indexes.len() - 1,
            feature_block: new_features.iter()
                .position(|&f| f == index[variable_i])
                .expect("missing feature that was just added"),
        };
        mapping.insert(densified, old);
    }

    let names = indexes.names()
        .iter()
        .filter(|&&name| name != variable)
        .cloned()
        .collect();
    let mut builder = IndexesBuilder::new(names);
    for env in new_indexes {
        builder.add(&env);
    }

    return RemovedResult {
        indexes: builder.finish(),
        new_features: new_features,
        mapping: mapping,
    };
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::test_systems;
    use crate::descriptor::indexes::{StructureSpeciesEnvironment, EnvironmentIndexes};
    use ndarray::array;

    fn do_prepare(gradients: bool) -> Descriptor {
        let mut systems = test_systems(&["water", "CH"]);

        let mut features = IndexesBuilder::new(vec!["foo", "bar", "baz"]);
        features.add(&[IndexValue::from(0_usize), IndexValue::from(1_isize), IndexValue::from(0.3)]);
        features.add(&[IndexValue::from(4_usize), IndexValue::from(2_isize), IndexValue::from(3.3)]);
        features.add(&[IndexValue::from(1_usize), IndexValue::from(0_isize), IndexValue::from(2.3)]);
        let features = features.finish();

        let environments = StructureSpeciesEnvironment;

        let mut descriptor = Descriptor::new();
        if gradients {
            let (environments, gradients) = environments.with_gradients(&mut systems.get());
            let gradients = gradients.unwrap();
            descriptor.prepare_gradients(environments, gradients, features);
        } else {
            let environments = environments.indexes(&mut systems.get());
            descriptor.prepare(environments, features);
        }

        return descriptor;
    }

    /// Convenience macro to create IndexValue
    macro_rules! v {
        ($value: expr) => {
            crate::descriptor::indexes::IndexValue::from($value as f64)
        };
    }

    #[test]
    fn prepare() {
        let descriptor = do_prepare(false);

        assert_eq!(descriptor.values.shape(), [4, 3]);

        assert_eq!(descriptor.environments.names(), ["structure", "species"]);
        assert_eq!(descriptor.environments[0], [v!(0), v!(1)]);
        assert_eq!(descriptor.environments[1], [v!(0), v!(123456)]);
        assert_eq!(descriptor.environments[2], [v!(1), v!(1)]);
        assert_eq!(descriptor.environments[3], [v!(1), v!(6)]);

        assert!(descriptor.gradients.is_none());
    }

    #[test]
    fn prepare_gradients() {
        let descriptor = do_prepare(true);

        let gradients = descriptor.gradients.unwrap();
        assert_eq!(gradients.shape(), [15, 3]);

        let gradients_indexes = descriptor.gradients_indexes.as_ref().unwrap();
        assert_eq!(gradients_indexes.names(), ["structure", "species", "atom", "spatial"]);

        // use a loop to simplify checking the spatial dimension
        let expected = [
            [v!(0), v!(1), v!(1)],
            [v!(0), v!(1), v!(2)],
            [v!(0), v!(123456), v!(0)],
            [v!(1), v!(1), v!(0)],
            [v!(1), v!(6), v!(1)]
        ];
        for (i, &value) in expected.iter().enumerate() {
            assert_eq!(gradients_indexes[3 * i][..3], value);
            assert_eq!(gradients_indexes[3 * i][3], v!(0));

            assert_eq!(gradients_indexes[3 * i + 1][..3], value);
            assert_eq!(gradients_indexes[3 * i + 1][3], v!(1));

            assert_eq!(gradients_indexes[3 * i + 2][..3], value);
            assert_eq!(gradients_indexes[3 * i + 2][3], v!(2));
        }
    }

    #[test]
    fn densify() {
        let mut descriptor = do_prepare(true);
        // environment indexes are checked in the tests above

        descriptor.values.assign(&array![
            [1.0, 2.0, 3.0],
            [4.0, 5.0, 6.0],
            [7.0, 8.0, 9.0],
            [10.0, 11.0, 12.0],
        ]);

        let gradients = descriptor.gradients.as_mut().unwrap();
        gradients.assign(&array![
            [1.0, 2.0, 3.0], [0.1, 0.2, 0.3], [-1.0, -2.0, -3.0],
            [4.0, 5.0, 6.0], [0.4, 0.5, 0.6], [-4.0, -5.0, -6.0],
            [7.0, 8.0, 9.0], [0.7, 0.8, 0.9], [-7.0, -8.0, -9.0],
            [10.0, 11.0, 12.0], [0.10, 0.11, 0.12], [-10.0, -11.0, -12.0],
            [13.0, 14.0, 15.0], [0.13, 0.14, 0.15], [-13.0, -14.0, -15.0],
        ]);

        // where the magic happens
        descriptor.densify("species");

        assert_eq!(descriptor.values.shape(), [2, 9]);
        assert_eq!(descriptor.environments.names(), ["structure"]);
        assert_eq!(descriptor.environments[0], [v!(0)]);
        assert_eq!(descriptor.environments[1], [v!(1)]);

        assert_eq!(descriptor.values, array![
            [1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 0.0, 0.0, 0.0],
            [7.0, 8.0, 9.0, 0.0, 0.0, 0.0, 10.0, 11.0, 12.0],
        ]);

        let gradients = descriptor.gradients.as_ref().unwrap();
        assert_eq!(gradients.shape(), [15, 9]);
        let gradients_indexes = descriptor.gradients_indexes.as_ref().unwrap();
        assert_eq!(gradients_indexes.names(), ["structure", "atom", "spatial"]);
        // use a loop to simplify checking the spatial dimension
        let expected = [
            [v!(0), v!(1)],
            [v!(0), v!(2)],
            [v!(0), v!(0)],
            [v!(1), v!(0)],
            [v!(1), v!(1)]
        ];
        for (i, &value) in expected.iter().enumerate() {
            assert_eq!(gradients_indexes[3 * i][..2], value);
            assert_eq!(gradients_indexes[3 * i][2], v!(0));

            assert_eq!(gradients_indexes[3 * i + 1][..2], value);
            assert_eq!(gradients_indexes[3 * i + 1][2], v!(1));

            assert_eq!(gradients_indexes[3 * i + 2][..2], value);
            assert_eq!(gradients_indexes[3 * i + 2][2], v!(2));
        }

        assert_eq!(*gradients, array![
            [/*H*/ 1.0, 2.0, 3.0,       /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 0.1, 0.2, 0.3,       /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ -1.0, -2.0, -3.0,    /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 4.0, 5.0, 6.0,       /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 0.4, 0.5, 0.6,       /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ -4.0, -5.0, -6.0,    /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 0.0, 0.0, 0.0,       /*O*/ 7.0, 8.0, 9.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 0.0, 0.0, 0.0,       /*O*/ 0.7, 0.8, 0.9,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 0.0, 0.0, 0.0,       /*O*/ -7.0, -8.0, -9.0, /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 10.0, 11.0, 12.0,    /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 0.1, 0.11, 0.12,     /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ -10.0, -11.0, -12.0, /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.0, 0.0, 0.0],
            [/*H*/ 0.0, 0.0, 0.0,       /*O*/ 0.0, 0.0, 0.0,    /*C*/ 13.0, 14.0, 15.0],
            [/*H*/ 0.0, 0.0, 0.0,       /*O*/ 0.0, 0.0, 0.0,    /*C*/ 0.13, 0.14, 0.15],
            [/*H*/ 0.0, 0.0, 0.0,       /*O*/ 0.0, 0.0, 0.0,    /*C*/ -13.0, -14.0, -15.0],
        ]);
    }
}