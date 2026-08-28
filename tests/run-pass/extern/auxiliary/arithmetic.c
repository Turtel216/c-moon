// A companion translation unit for the `extern` fixtures, built with GCC and
// linked in. Nothing here is ever seen by the compiler under test: what the
// fixtures check is that the two agree about the ABI at the link boundary.

int triple(int x) {
    return x * 3;
}

// Eight parameters, so the last two arrive on the stack rather than in
// registers -- the half of the calling convention a same-file call would
// exercise against this compiler's own conventions rather than the ABI's.
int sum8(int a, int b, int c, int d, int e, int f, int g, int h) {
    return a + b + c + d + e + f + g + h;
}

// Widths other than `int` cross the boundary here: a `char` argument and a
// `long int` result.
long widen(char c) {
    return (long) c * 1000000000;
}
