// `long int` arithmetic uses the full 64-bit registers, which is the whole
// difference between it and `int`.

// CHECK-LABEL: sum:
// CHECK: add r
// CHECK-NOT: add e

long int sum(long int a, long int b) {
    return a + b;
}

int main() {
    return sum(20, 22);
}
