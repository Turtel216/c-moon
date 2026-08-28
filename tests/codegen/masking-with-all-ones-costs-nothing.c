//@ compile-flags: --opt

// A mask of ones selects every bit there is and a shift by nothing moves
// nothing, so neither of them reaches the assembly.

// CHECK-LABEL: identity:
// CHECK-NOT: and
// CHECK-NOT: shl
// CHECK-NOT: sar

int identity(int a) {
    return (a & -1) << 0;
}

int main() {
    return identity(42);
}
