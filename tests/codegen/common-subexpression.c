// Value numbering recognises the second `a + b` as the first one, so only one
// addition survives into the emitted code. The operands are parameters, so
// there is nothing here for constant folding to do instead.
//@ compile-flags: --opt

// CHECK-LABEL: f:
// CHECK: add
// CHECK-NOT: add
// CHECK: imul

int f(int a, int b) {
    return (a + b) * (a + b);
}

int main() {
    return f(2, 3);
}
