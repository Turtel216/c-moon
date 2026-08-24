// `!x` is the comparison `x == 0` written another way, so it selects a single
// compare-and-set and needs no branch at all -- and, being a comparison, it
// is made at the operand's own width.

// CHECK-LABEL: is_zero:
// CHECK: cmp r
// CHECK-NEXT: sete
// CHECK-NOT: cmp e
// CHECK-NOT: jne

int is_zero(long int value) {
    return !value;
}

int main() {
    return is_zero(3);
}
