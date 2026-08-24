// An unsigned division is `div` rather than `idiv`, and the high half of the
// dividend is cleared rather than filled with the dividend's sign.

// CHECK-LABEL: divide:
// CHECK: mov edx, 0
// CHECK-NEXT: div
// CHECK-NOT: idiv
// CHECK-NOT: cdq

unsigned int divide(unsigned int dividend, unsigned int divisor) {
    return dividend / divisor;
}

int main() {
    return divide(10, 3);
}
