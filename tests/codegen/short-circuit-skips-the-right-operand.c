// The right operand of `&&` sits in a block of its own that the left one
// branches over, which is what lets the call not happen at all. Without the
// branch the call would be reached however `zero` came out.

// CHECK-LABEL: main:
// CHECK: test
// CHECK-NEXT: jne
// CHECK: call side

int side(int *counter) {
    *counter = *counter + 1;
    return 1;
}

int main() {
    int calls = 0;
    int zero = 0;

    return (zero && side(&calls)) + calls;
}
