// A `char` promotes to an `int` before any arithmetic, and the promotion sign
// extends it: plain `char` is signed, so the bits above the byte have to be
// manufactured from its top bit rather than zeroed.

// CHECK-LABEL: sum:
// CHECK: movsx e

char sum(char a, char b) {
    return a + b;
}

int main() {
    return sum(1, 2);
}
