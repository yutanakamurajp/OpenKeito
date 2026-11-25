//! Dense LU factorization with implicit scaled partial pivoting, for real
//! matrices (time steps) and complex ones (phasor solutions and frequency
//! scans).
//!
//! `Solver` puts it and the sparse LU (sparse.rs) behind the same two calls
//! (`factor`, `solve`).

use std::ops::{Add, Div, Mul, Neg, Sub};

pub use num_complex::Complex64;

/// The numbers a matrix can hold: f64 or Complex64.
pub trait Scalar:
    Copy
    + PartialEq
    + std::fmt::Debug
    + Add<Output = Self>
    + Sub<Output = Self>
    + Mul<Output = Self>
    + Div<Output = Self>
    + Neg<Output = Self>
{
    const ZERO: Self;
    const ONE: Self;
    /// |x|.
    fn magnitude(self) -> f64;
    /// x times a real number.
    fn scale(self, s: f64) -> Self;
    fn is_finite(self) -> bool;
}

impl Scalar for f64 {
    const ZERO: f64 = 0.0;
    const ONE: f64 = 1.0;
    fn magnitude(self) -> f64 {
        self.abs()
    }
    fn scale(self, s: f64) -> f64 {
        self * s
    }
    fn is_finite(self) -> bool {
        f64::is_finite(self)
    }
}

impl Scalar for Complex64 {
    const ZERO: Complex64 = Complex64::new(0.0, 0.0);
    const ONE: Complex64 = Complex64::new(1.0, 0.0);
    fn magnitude(self) -> f64 {
        self.norm()
    }
    fn scale(self, s: f64) -> Complex64 {
        self * s
    }
    fn is_finite(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
}

/// Receives the entries of a system matrix. Elements write into it without
/// knowing how the matrix is stored.
pub trait Stamp<T = f64> {
    /// Adds `value` to the entry at `row`, `col`.
    fn add(&mut self, row: usize, col: usize, value: T);
}

impl<T: Scalar> Stamp<T> for DenseMatrix<T> {
    fn add(&mut self, row: usize, col: usize, value: T) {
        DenseMatrix::add(self, row, col, value);
    }
}

/// Passes real entries on to a complex matrix.
pub struct RealPart<'a>(pub &'a mut dyn Stamp<Complex64>);

impl Stamp for RealPart<'_> {
    fn add(&mut self, row: usize, col: usize, value: f64) {
        self.0.add(row, col, Complex64::new(value, 0.0));
    }
}

/// Square matrix, row-major.
#[derive(Debug, Clone)]
pub struct DenseMatrix<T = f64> {
    n: usize,
    a: Vec<T>,
}

impl<T: Scalar> DenseMatrix<T> {
    pub fn zeros(n: usize) -> Self {
        DenseMatrix {
            n,
            a: vec![T::ZERO; n * n],
        }
    }

    pub fn n(&self) -> usize {
        self.n
    }

    pub fn add(&mut self, row: usize, col: usize, value: T) {
        let k = row * self.n + col;
        self.a[k] = self.a[k] + value;
    }

    pub fn get(&self, row: usize, col: usize) -> T {
        self.a[row * self.n + col]
    }
}

/// LU factors of a matrix, P*A = L*U.
#[derive(Debug, Clone)]
pub struct Lu<T = f64> {
    n: usize,
    lu: Vec<T>,
    perm: Vec<usize>,
    /// Smallest pivot relative to the largest entry of its row.
    min_scaled_pivot: f64,
}

/// A pivot smaller than this, relative to the largest entry of its row in the
/// original matrix, means the matrix is singular to working precision. The
/// dense and the sparse LU share it.
pub(crate) const SINGULAR_THRESHOLD: f64 = 1e-13;

impl<T: Scalar> Lu<T> {
    /// Factorizes `m`. On a singular matrix returns the index of the unknown
    /// (column) that cannot be determined.
    pub fn factor(m: &DenseMatrix<T>) -> Result<Lu<T>, usize> {
        let n = m.n;
        let mut a = m.a.clone();
        let mut perm: Vec<usize> = (0..n).collect();
        let mut scale = vec![0.0; n];
        let mut min_scaled_pivot = f64::INFINITY;
        for i in 0..n {
            let max = a[i * n..(i + 1) * n]
                .iter()
                .fold(0.0_f64, |acc, v| acc.max(v.magnitude()));
            if max == 0.0 || !max.is_finite() {
                return Err(i);
            }
            scale[i] = 1.0 / max;
        }
        let mut col_max = vec![0.0_f64; n];
        for i in 0..n {
            let s = scale[i];
            for j in 0..n {
                col_max[j] = col_max[j].max(s * a[i * n + j].magnitude());
            }
        }
        for c in &mut col_max {
            *c = c.min(1.0);
        }
        for k in 0..n {
            let mut best = 0.0;
            let mut p = k;
            for i in k..n {
                let v = scale[i] * a[i * n + k].magnitude();
                if v > best {
                    best = v;
                    p = i;
                }
            }
            if col_max[k] == 0.0 || best < SINGULAR_THRESHOLD * col_max[k] || best.is_nan() {
                return Err(k);
            }
            let scaled_pivot = if best >= SINGULAR_THRESHOLD {
                best
            } else {
                best / col_max[k]
            };
            min_scaled_pivot = min_scaled_pivot.min(scaled_pivot);
            if p != k {
                for j in 0..n {
                    a.swap(k * n + j, p * n + j);
                }
                scale.swap(k, p);
                perm.swap(k, p);
            }
            let pivot = a[k * n + k];
            for i in k + 1..n {
                let f = a[i * n + k] / pivot;
                a[i * n + k] = f;
                if f != T::ZERO {
                    for j in k + 1..n {
                        a[i * n + j] = a[i * n + j] - f * a[k * n + j];
                    }
                }
            }
        }
        Ok(Lu {
            n,
            lu: a,
            perm,
            min_scaled_pivot,
        })
    }

    /// Rough condition number: the reciprocal of the smallest scaled pivot.
    /// Rounding errors in a solution are about this many times the machine
    /// epsilon, relative to its largest entries.
    pub fn condition_estimate(&self) -> f64 {
        if self.n == 0 {
            1.0
        } else {
            1.0 / self.min_scaled_pivot
        }
    }

    /// Solves A*x = b. `b` is the right-hand side, `x` receives the solution.
    #[inline]
    pub fn solve(&self, b: &[T], x: &mut [T]) {
        let n = self.n;
        for i in 0..n {
            let mut s = b[self.perm[i]];
            for j in 0..i {
                s = s - self.lu[i * n + j] * x[j];
            }
            x[i] = s;
        }
        for i in (0..n).rev() {
            let mut s = x[i];
            for j in i + 1..n {
                s = s - self.lu[i * n + j] * x[j];
            }
            x[i] = s / self.lu[i * n + i];
        }
    }
}

/// LU factors from either solver.
pub enum Factors<T: Scalar + faer::traits::ComplexField> {
    Dense(Lu<T>),
    /// Boxed: the sparse factors are much larger than the dense handle.
    Sparse(Box<crate::sparse::SparseLu<T>>),
}

impl<T: Scalar + faer::traits::ComplexField> Factors<T> {
    #[inline]
    pub fn solve(&self, b: &[T], x: &mut [T]) {
        match self {
            Factors::Dense(lu) => lu.solve(b, x),
            Factors::Sparse(lu) => lu.solve(b, x),
        }
    }

    pub fn condition_estimate(&self) -> f64 {
        match self {
            Factors::Dense(lu) => lu.condition_estimate(),
            Factors::Sparse(lu) => lu.condition_estimate(),
        }
    }
}

/// The linear solver of a circuit: dense, or sparse with the column order
/// of its pattern.
pub enum Solver {
    Dense(usize),
    Sparse(crate::sparse::SparseSymbolic),
}

impl Solver {
    /// A solver for `n` unknowns. For the sparse one, `structure` writes
    /// every entry that any state of the circuit can have.
    pub fn new(sparse: bool, n: usize, structure: impl FnOnce(&mut dyn Stamp)) -> Solver {
        if !sparse {
            return Solver::Dense(n);
        }
        let mut t = crate::sparse::Triplets {
            entries: Vec::new(),
        };
        structure(&mut t);
        let pattern: Vec<(usize, usize)> = t.entries.iter().map(|&(r, c, _)| (r, c)).collect();
        Solver::Sparse(crate::sparse::SparseSymbolic::new(n, &pattern))
    }

    pub fn name(&self) -> &'static str {
        match self {
            Solver::Dense(_) => "dense",
            Solver::Sparse(_) => "sparse",
        }
    }

    /// Factorizes the matrix that `assemble` writes. On a singular matrix,
    /// returns the index of the unknown that cannot be determined.
    pub fn factor<T: Scalar + faer::traits::ComplexField>(
        &self,
        assemble: impl FnOnce(&mut dyn Stamp<T>),
    ) -> Result<Factors<T>, usize> {
        match self {
            Solver::Dense(n) => {
                let mut m = DenseMatrix::<T>::zeros(*n);
                assemble(&mut m);
                Lu::factor(&m).map(Factors::Dense)
            }
            Solver::Sparse(symbolic) => {
                let mut t = crate::sparse::Triplets {
                    entries: Vec::with_capacity(8 * symbolic.n()),
                };
                assemble(&mut t);
                crate::sparse::SparseLu::factor(symbolic, &t.entries)
                    .map(|lu| Factors::Sparse(Box::new(lu)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solves_a_system_that_needs_pivoting() {
        let mut m = DenseMatrix::zeros(3);
        let rows = [[0.0, 2.0, 1.0], [1.0, 1.0, 1.0], [2.0, 1.0, 3.0]];
        for (i, r) in rows.iter().enumerate() {
            for (j, v) in r.iter().enumerate() {
                m.add(i, j, *v);
            }
        }
        let lu = Lu::factor(&m).unwrap();
        let expected = [1.0, -2.0, 3.0];
        let b: Vec<f64> = rows
            .iter()
            .map(|r| r.iter().zip(expected).map(|(a, x)| a * x).sum())
            .collect();
        let mut x = [0.0; 3];
        lu.solve(&b, &mut x);
        for (a, e) in x.iter().zip(expected) {
            assert!((a - e).abs() < 1e-12, "{x:?}");
        }
    }

    #[test]
    fn reports_the_undetermined_unknown() {
        let mut m = DenseMatrix::zeros(2);
        m.add(0, 0, 1.0);
        m.add(1, 0, 2.0);
        assert_eq!(Lu::factor(&m).unwrap_err(), 1);
    }

    #[test]
    fn badly_scaled_rows_are_not_called_singular() {
        let mut m = DenseMatrix::zeros(2);
        m.add(0, 0, 1e9);
        m.add(0, 1, -1e9);
        m.add(1, 0, -1e-9);
        m.add(1, 1, 3e-9);
        let lu = Lu::factor(&m).unwrap();
        let mut x = [0.0; 2];
        lu.solve(&[0.0, 2e-9], &mut x);
        assert!((x[0] - 1.0).abs() < 1e-9 && (x[1] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn solves_a_complex_system() {
        let j = Complex64::new(0.0, 1.0);
        let one = Complex64::new(1.0, 0.0);
        let rows = [[one + j, -j], [-j, 2.0 * one + 0.5 * j]];
        let mut m = DenseMatrix::<Complex64>::zeros(2);
        for (i, r) in rows.iter().enumerate() {
            for (k, v) in r.iter().enumerate() {
                m.add(i, k, *v);
            }
        }
        let expected = [Complex64::new(1.0, 2.0), Complex64::new(-3.0, 0.5)];
        let b: Vec<Complex64> = rows
            .iter()
            .map(|r| r[0] * expected[0] + r[1] * expected[1])
            .collect();
        let mut x = [Complex64::ZERO; 2];
        Lu::factor(&m).unwrap().solve(&b, &mut x);
        for (a, e) in x.iter().zip(expected) {
            assert!((a - e).norm() < 1e-14, "{x:?}");
        }
    }
}
