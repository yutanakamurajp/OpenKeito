//! Sparse LU with the same contract as the dense one: implicit scaled
//! partial pivoting, a singular pivot below 1e-13 of its row, and the
//! smallest scaled pivot as a condition estimate.
//!
//! It uses the left-looking (Gilbert-Peierls) LU with partial pivoting of
//! the faer crate, with the fill-reducing column order COLAMD computed once
//! for the pattern of every state of the circuit (the symbolic step). The
//! numeric factorization is repeated when values change; it picks its pivots
//! again each time. Every row is divided by its largest entry before the
//! factorization (row equilibration), so that pivoting by magnitude equals
//! the scaled pivoting of the dense LU and the smallest |u_kk| is the
//! smallest scaled pivot.

use std::cell::RefCell;

use faer::dyn_stack::{MemBuffer, MemStack, StackReq};
use faer::perm::PermRef;
use faer::sparse::linalg::colamd;
use faer::sparse::linalg::lu::simplicial;
use faer::sparse::{SparseColMat, Triplet};
use faer::traits::ComplexField;
use faer::{Conj, MatMut, Par};

use crate::linalg::{Scalar, Stamp, SINGULAR_THRESHOLD};

/// Collects the entries of a matrix; duplicates are summed.
pub struct Triplets<T> {
    pub entries: Vec<(usize, usize, T)>,
}

impl<T: Scalar> Stamp<T> for Triplets<T> {
    fn add(&mut self, row: usize, col: usize, value: T) {
        self.entries.push((row, col, value));
    }
}

/// The column order of a pattern (the symbolic step). Computed once.
#[derive(Debug, Clone)]
pub struct SparseSymbolic {
    n: usize,
    col_fwd: Vec<usize>,
    col_inv: Vec<usize>,
}

impl SparseSymbolic {
    /// COLAMD order of the pattern (row, column) of an n x n matrix.
    pub fn new(n: usize, pattern: &[(usize, usize)]) -> SparseSymbolic {
        let mut col_fwd = vec![0usize; n];
        let mut col_inv = vec![0usize; n];
        if n > 0 {
            let triplets: Vec<Triplet<usize, usize, f64>> = pattern
                .iter()
                .map(|&(r, c)| Triplet::new(r, c, 1.0))
                .collect();
            let a = SparseColMat::<usize, f64>::try_new_from_triplets(n, n, &triplets)
                .expect("pattern indices are within the matrix");
            let nnz = a.symbolic().compute_nnz();
            let mut mem = MemBuffer::new(colamd::order_scratch::<usize>(n, n, nnz));
            colamd::order(
                &mut col_fwd,
                &mut col_inv,
                a.symbolic(),
                colamd::Control::default(),
                MemStack::new(&mut mem),
            )
            .expect("the column order of a valid pattern");
        }
        SparseSymbolic {
            n,
            col_fwd,
            col_inv,
        }
    }

    pub fn n(&self) -> usize {
        self.n
    }
}

/// LU factors of a sparse matrix.
pub struct SparseLu<T: ComplexField> {
    n: usize,
    col_fwd: Vec<usize>,
    col_inv: Vec<usize>,
    row_fwd: Vec<usize>,
    row_inv: Vec<usize>,
    lu: simplicial::SimplicialLu<usize, T>,
    /// 1 / largest entry of each row.
    scale: Vec<f64>,
    min_scaled_pivot: f64,
    /// Workspace of the solutions.
    work: RefCell<MemBuffer>,
}

impl<T: Scalar + ComplexField> SparseLu<T> {
    /// Factorizes the matrix given by its entries (duplicates are summed).
    /// On a singular matrix returns the index of the unknown (column) that
    /// cannot be determined.
    pub fn factor(symbolic: &SparseSymbolic, entries: &[(usize, usize, T)]) -> Result<Self, usize> {
        let n = symbolic.n;
        let triplets: Vec<Triplet<usize, usize, T>> = entries
            .iter()
            .map(|&(r, c, v)| Triplet::new(r, c, v))
            .collect();
        let mut a = SparseColMat::<usize, T>::try_new_from_triplets(n, n, &triplets)
            .expect("entries are within the matrix");
        // Row equilibration.
        let rows: Vec<usize> = a.symbolic().row_idx().to_vec();
        let mut largest = vec![0.0_f64; n];
        for (&r, v) in rows.iter().zip(a.val()) {
            largest[r] = largest[r].max(Scalar::magnitude(*v));
        }
        let mut scale = vec![0.0; n];
        for (i, m) in largest.iter().enumerate() {
            if *m == 0.0 || !m.is_finite() {
                return Err(i);
            }
            scale[i] = 1.0 / m;
        }
        for (&r, v) in rows.iter().zip(a.val_mut()) {
            *v = Scalar::scale(*v, scale[r]);
        }
        let mut col_max = vec![0.0_f64; n];
        {
            let ptr = a.symbolic().col_ptr();
            let val = a.val();
            for c in 0..n {
                let mut m = 0.0_f64;
                for &v in &val[ptr[c]..ptr[c + 1]] {
                    m = m.max(Scalar::magnitude(v));
                }
                col_max[c] = m.min(1.0);
            }
        }

        let mut row_fwd = vec![0usize; n];
        let mut row_inv = vec![0usize; n];
        let mut lu = simplicial::SimplicialLu::<usize, T>::new();
        let mut mem = MemBuffer::new(StackReq::any_of(&[
            simplicial::factorize_simplicial_numeric_lu_scratch::<usize, T>(n, n),
            simplicial::solve_in_place_scratch::<usize, T>(n, 1, Par::Seq),
        ]));
        let col_perm = PermRef::<'_, usize>::new_checked(&symbolic.col_fwd, &symbolic.col_inv, n);
        if let Err(e) = simplicial::factorize_simplicial_numeric_lu(
            &mut row_fwd,
            &mut row_inv,
            &mut lu,
            a.as_ref(),
            col_perm,
            MemStack::new(&mut mem),
        ) {
            return Err(match e {
                faer::sparse::linalg::LuError::SymbolicSingular { index } => {
                    symbolic.col_fwd[index.min(n - 1)]
                }
                faer::sparse::linalg::LuError::Generic(_) => 0,
            });
        }
        // The diagonal of U: in column j (of the permuted matrix) the entry
        // with row index j. Column j is column col_fwd[j] of the matrix.
        let mut min_scaled_pivot = f64::INFINITY;
        {
            let u = lu.u_factor_unsorted();
            let ptr = u.symbolic().col_ptr();
            let idx = u.symbolic().row_idx();
            let val = u.val();
            for j in 0..n {
                let pivot = (ptr[j]..ptr[j + 1])
                    .find(|&p| idx[p] == j)
                    .map_or(0.0, |p| Scalar::magnitude(val[p]));
                let col = symbolic.col_fwd[j];
                let cm = col_max[col];
                if cm == 0.0 || pivot < SINGULAR_THRESHOLD * cm || pivot.is_nan() {
                    return Err(col);
                }
                let scaled_pivot = if pivot >= SINGULAR_THRESHOLD {
                    pivot
                } else {
                    pivot / cm
                };
                min_scaled_pivot = min_scaled_pivot.min(scaled_pivot);
            }
        }
        Ok(SparseLu {
            n,
            col_fwd: symbolic.col_fwd.clone(),
            col_inv: symbolic.col_inv.clone(),
            row_fwd,
            row_inv,
            lu,
            scale,
            min_scaled_pivot,
            work: RefCell::new(mem),
        })
    }

    /// Rough condition number: the reciprocal of the smallest scaled pivot.
    pub fn condition_estimate(&self) -> f64 {
        if self.n == 0 {
            1.0
        } else {
            1.0 / self.min_scaled_pivot
        }
    }

    /// Solves A*x = b.
    pub fn solve(&self, b: &[T], x: &mut [T]) {
        let mut mem = self.work.borrow_mut();
        for i in 0..self.n {
            x[i] = Scalar::scale(b[i], self.scale[i]);
        }
        let rhs = MatMut::from_column_major_slice_mut(x, self.n, 1);
        let row_perm = PermRef::<'_, usize>::new_checked(&self.row_fwd, &self.row_inv, self.n);
        let col_perm = PermRef::<'_, usize>::new_checked(&self.col_fwd, &self.col_inv, self.n);
        self.lu.solve_in_place_with_conj(
            row_perm,
            col_perm,
            Conj::No,
            rhs,
            Par::Seq,
            MemStack::new(&mut mem),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linalg::{Complex64, DenseMatrix, Lu};

    fn pattern(entries: &[(usize, usize, f64)]) -> Vec<(usize, usize)> {
        entries.iter().map(|&(r, c, _)| (r, c)).collect()
    }

    #[test]
    fn solves_like_the_dense_lu() {
        // A three-phase ladder: coupled 3x3 blocks between neighbouring
        // sections, as in a line of pi sections.
        let sections = 40;
        let n = 3 * sections;
        let mut e = Vec::new();
        for k in 0..sections {
            for p in 0..3 {
                for q in 0..3 {
                    let g = if p == q { 2.0 } else { -0.3 };
                    let (i, j) = (3 * k + p, 3 * k + q);
                    e.push((i, j, g + if p == q { 0.01 } else { 0.0 }));
                    if k + 1 < sections {
                        let (i2, j2) = (3 * (k + 1) + p, 3 * (k + 1) + q);
                        e.push((i2, j2, g));
                        e.push((i, j2, -g));
                        e.push((i2, j, -g));
                    }
                }
            }
        }
        let symbolic = SparseSymbolic::new(n, &pattern(&e));
        let sparse = SparseLu::factor(&symbolic, &e).unwrap();
        let mut d = DenseMatrix::zeros(n);
        for &(r, c, v) in &e {
            d.add(r, c, v);
        }
        let dense = Lu::factor(&d).unwrap();
        let b: Vec<f64> = (0..n).map(|i| (i as f64).sin()).collect();
        let (mut xs, mut xd) = (vec![0.0; n], vec![0.0; n]);
        sparse.solve(&b, &mut xs);
        dense.solve(&b, &mut xd);
        let scale = xd.iter().fold(0.0_f64, |a, v| a.max(v.abs()));
        let diff = xs
            .iter()
            .zip(&xd)
            .fold(0.0_f64, |a, (s, d)| a.max((s - d).abs()));
        assert!(diff < 1e-12 * scale, "{diff:e} of {scale:e}");
        // Both condition estimates are reciprocals of scaled pivots.
        assert!(sparse.condition_estimate() >= 1.0 && sparse.condition_estimate().is_finite());
    }

    #[test]
    fn badly_scaled_rows_are_not_called_singular() {
        let e = [(0, 0, 1e9), (0, 1, -1e9), (1, 0, -1e-9), (1, 1, 3e-9)];
        let lu = SparseLu::factor(&SparseSymbolic::new(2, &pattern(&e)), &e).unwrap();
        let mut x = [0.0; 2];
        lu.solve(&[0.0, 2e-9], &mut x);
        assert!(
            (x[0] - 1.0).abs() < 1e-9 && (x[1] - 1.0).abs() < 1e-9,
            "{x:?}"
        );
    }

    #[test]
    fn reports_the_undetermined_unknown() {
        // Column 1 is zero in every row.
        let e = [(0, 0, 1.0), (1, 0, 2.0), (1, 1, 0.0)];
        let err = SparseLu::factor(&SparseSymbolic::new(2, &pattern(&e)), &e).err();
        assert_eq!(err, Some(1));
        // Two equal rows: the second pivot vanishes.
        let e = [(0, 0, 1.0), (0, 1, 1.0), (1, 0, 1.0), (1, 1, 1.0)];
        assert!(SparseLu::factor(&SparseSymbolic::new(2, &pattern(&e)), &e).is_err());
    }

    #[test]
    fn solves_a_complex_system() {
        let j = Complex64::new(0.0, 1.0);
        let one = Complex64::new(1.0, 0.0);
        let e = [
            (0, 0, one + j),
            (0, 1, -j),
            (1, 0, -j),
            (1, 1, 2.0 * one + 0.5 * j),
        ];
        let p: Vec<(usize, usize)> = e.iter().map(|&(r, c, _)| (r, c)).collect();
        let lu = SparseLu::factor(&SparseSymbolic::new(2, &p), &e).unwrap();
        let x0 = [Complex64::new(1.0, 2.0), Complex64::new(-3.0, 0.5)];
        let b = [
            e[0].2 * x0[0] + e[1].2 * x0[1],
            e[2].2 * x0[0] + e[3].2 * x0[1],
        ];
        let mut x = [Complex64::new(0.0, 0.0); 2];
        lu.solve(&b, &mut x);
        for (a, b) in x.iter().zip(x0) {
            assert!((a - b).norm() < 1e-14, "{x:?}");
        }
    }
}
