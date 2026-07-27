// Without `--opt` the folding passes do not run, so the addition survives
// into the emitted code. This is the counterpart to `constant-fold-add.c`
// and keeps the optimisation tests honest.

// CHECK-LABEL: main:
// CHECK: add

int main() {
    int a = 20;
    int b = 22;
    return a + b;
}
