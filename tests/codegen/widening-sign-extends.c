// Widening an `int` to a `long int` sign extends it: only the low 32 bits of
// an `int` mean anything, so the rest have to be manufactured.

// CHECK-LABEL: main:
// CHECK: movsx r

int main() {
    int narrow = 42;
    long int wide = narrow;
    return wide / 42;
}
