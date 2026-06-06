use std::ptr::NonNull;

use crate::libsais::LibsaisError;

#[allow(unused)]
unsafe extern "C" {
    /**
     * Constructs the generalized suffix array (GSA) of given string set.
     * @param T [0..n-1] The input string set using 0 as separators (T[n-1] must be 0).
     * @param SA [0..n-1+fs] The output array of suffixes.
     * @param n The length of the given string set.
     * @param fs The extra space available at the end of SA array (0 should be enough for most cases).
     * @param freq [0..255] The output symbol frequency table (can be NULL).
     * @return 0 if no error occurred, -1 or -2 otherwise.
     */
    unsafe fn libsais_gsa(
        T: *const u8,
        SA: *mut i32,
        n: i32,
        fs: i32,
        freq: Option<NonNull<i32>>,
    ) -> i32;

    /**
     * Constructs the permuted longest common prefix array (PLCP) of a given string and a suffix array.
     * @param T [0..n-1] The input string.
     * @param SA [0..n-1] The input suffix array.
     * @param PLCP [0..n-1] The output permuted longest common prefix array.
     * @param n The length of the string and the suffix array.
     * @return 0 if no error occurred, -1 otherwise.
     */
    unsafe fn libsais_plcp_gsa(T: *const u8, SA: *const i32, PLCP: *mut i32, n: i32) -> i32;

    /**
     * Constructs the longest common prefix array (LCP) of a given permuted longest common prefix array (PLCP) and a suffix array.
     * @param PLCP [0..n-1] The input permuted longest common prefix array.
     * @param SA [0..n-1] The input suffix array or generalized suffix array (GSA).
     * @param LCP [0..n-1] The output longest common prefix array (can be SA).
     * @param n The length of the permuted longest common prefix array and the suffix array.
     * @return 0 if no error occurred, -1 otherwise.
     */
    unsafe fn libsais_lcp(PLCP: *const i32, SA: *const i32, LCP: *mut i32, n: i32) -> i32;

    /**
     * Constructs the generalized suffix array (GSA) of given string set in parallel using OpenMP.
     * @param T [0..n-1] The input string set using 0 as separators (T[n-1] must be 0).
     * @param SA [0..n-1+fs] The output array of suffixes.
     * @param n The length of the given string set.
     * @param fs The extra space available at the end of SA array (0 should be enough for most cases).
     * @param freq [0..255] The output symbol frequency table (can be NULL).
     * @param threads The number of OpenMP threads to use (can be 0 for OpenMP default).
     * @return 0 if no error occurred, -1 or -2 otherwise.
     */
    #[cfg(feature = "libsais_omp")]
    unsafe fn libsais_gsa_omp(
        T: *const u8,
        SA: *mut i32,
        n: i32,
        fs: i32,
        freq: Option<NonNull<i32>>,
        threads: i32,
    ) -> i32;

    /**
     * Constructs the permuted longest common prefix array (PLCP) of a given string set and a generalized suffix array (GSA) in parallel using OpenMP.
     * @param T [0..n-1] The input string set using 0 as separators (T[n-1] must be 0).
     * @param SA [0..n-1] The input generalized suffix array.
     * @param PLCP [0..n-1] The output permuted longest common prefix array.
     * @param n The length of the string set and the generalized suffix array.
     * @param threads The number of OpenMP threads to use (can be 0 for OpenMP default).
     * @return 0 if no error occurred, -1 otherwise.
     */
    #[cfg(feature = "libsais_omp")]
    unsafe fn libsais_plcp_gsa_omp(
        T: *const u8,
        SA: *const i32,
        PLCP: *mut i32,
        n: i32,
        threads: i32,
    ) -> i32;

    /**
     * Constructs the longest common prefix array (LCP) of a given permuted longest common prefix array (PLCP) and a suffix array in parallel using OpenMP.
     * @param PLCP [0..n-1] The input permuted longest common prefix array.
     * @param SA [0..n-1] The input suffix array or generalized suffix array (GSA).
     * @param LCP [0..n-1] The output longest common prefix array (can be SA).
     * @param n The length of the permuted longest common prefix array and the suffix array.
     * @param threads The number of OpenMP threads to use (can be 0 for OpenMP default).
     * @return 0 if no error occurred, -1 otherwise.
     */
    #[cfg(feature = "libsais_omp")]
    unsafe fn libsais_lcp_omp(
        PLCP: *const i32,
        SA: *const i32,
        LCP: *mut i32,
        n: i32,
        threads: i32,
    ) -> i32;
}

#[cfg(feature = "libsais_omp")]
const THREADS: i32 = 4;

/// Constructs the generalized suffix array (GSA) of given string set.
///
/// T [0..n-1] The input string set using 0 as separators (T[n-1] must be 0).
/// SA [0..n-1+fs] The output array of suffixes.
/// n The length of the given string set.
/// fs The extra space available at the end of SA array (0 should be enough for most cases).
/// freq [0..255] The output symbol frequency table (can be NULL).
#[allow(non_snake_case)]
pub unsafe fn gsa(
    T: *const u8,
    SA: *mut i32,
    n: i32,
    fs: i32,
    freq: Option<NonNull<i32>>,
) -> Result<(), LibsaisError> {
    cfg_select! {
        not(feature = "libsais_omp") => unsafe {
            to_result(libsais_gsa(T, SA, n, fs, freq))
        }
        _ => unsafe {
            to_result(libsais_gsa_omp(T, SA, n, fs, freq, THREADS))
        }
    }
}

/// Constructs the permuted longest common prefix array (PLCP) of a given string and a suffix array.
///
/// T [0..n-1] The input string.
/// SA [0..n-1] The input suffix array.
/// PLCP [0..n-1] The output permuted longest common prefix array.
/// n The length of the string and the suffix array.
#[allow(non_snake_case)]
pub unsafe fn plcp_gsa(
    T: *const u8,
    SA: *const i32,
    PLCP: *mut i32,
    n: i32,
) -> Result<(), LibsaisError> {
    cfg_select! {
        not(feature = "libsais_omp") => unsafe {
            to_result(libsais_plcp_gsa(T, SA, PLCP, n))
        }
        _ => unsafe {
            to_result(libsais_plcp_gsa_omp(T, SA, PLCP, n, THREADS))
        }
    }
}

/// Constructs the longest common prefix array (LCP) of a given permuted longest common prefix array (PLCP) and a suffix array.
///
/// PLCP [0..n-1] The input permuted longest common prefix array.
/// SA [0..n-1] The input suffix array or generalized suffix array (GSA).
/// LCP [0..n-1] The output longest common prefix array (can be SA).
/// n The length of the permuted longest common prefix array and the suffix array.
#[allow(non_snake_case)]
pub unsafe fn lcp(
    PLCP: *const i32,
    SA: *const i32,
    LCP: *mut i32,
    n: i32,
) -> Result<(), LibsaisError> {
    cfg_select! {
        not(feature = "libsais_omp") => unsafe {
            to_result(libsais_lcp(PLCP, SA, LCP, n))
        }
        _ => unsafe {
            to_result(libsais_lcp_omp(PLCP, SA, LCP, n, THREADS))
        }
    }
}

fn to_result(code: i32) -> Result<(), LibsaisError> {
    match code {
        0 => Ok(()),
        -1 => Err(LibsaisError::InvalidParameter),
        code => Err(LibsaisError::Internal(code)),
    }
}
