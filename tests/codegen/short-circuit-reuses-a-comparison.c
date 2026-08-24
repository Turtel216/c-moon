// The right operand of `&&` has to answer 0 or 1, which a comparison already
// does, so it is used as it stands instead of being compared against zero a
// second time.

// CHECK-LABEL: both:
// CHECK: setl
// CHECK-NOT: setne

int both(int a, int b, int c) {
    return a && b < c;
}

int main() {
    return both(1, 2, 3);
}
