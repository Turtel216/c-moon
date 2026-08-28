// A companion that calls *back* into the fixture, so the link boundary is
// crossed in both directions: GCC-compiled code calling a function this
// compiler emitted.

// Defined by the fixture, and compiled by the compiler under test.
int doubled(int x);

int twice_doubled(int x) {
    return doubled(doubled(x));
}
