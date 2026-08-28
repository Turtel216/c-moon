// The one shift whose answer depends on how its left operand reads: an `int`
// keeps its sign, which `sar` copies down, where an `unsigned int` takes
// zeroes in at the top, which is `shr`.

// CHECK-LABEL: signed_shift:
// CHECK: sar
// CHECK-NOT: shr
// CHECK-LABEL: unsigned_shift:
// CHECK: shr
// CHECK-NOT: sar

int signed_shift(int a, int b) {
    return a >> b;
}

unsigned int unsigned_shift(unsigned int a, int b) {
    return a >> b;
}

int main() {
    return signed_shift(8, 1) + (int) unsigned_shift(8, 1);
}
