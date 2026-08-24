// An unsigned ordering is decided by the carry flag, so it is `setb` rather
// than the `setl` a signed one uses.

// CHECK-LABEL: below:
// CHECK: setb
// CHECK-NOT: setl

int below(unsigned int left, unsigned int right) {
    return left < right;
}

int main() {
    return below(1, 2);
}
