// An `int` is a 32-bit type, so its arithmetic uses the 32-bit registers and
// wraps where the language says it does. The value still travels to the
// caller in the full return register.

// CHECK-LABEL: main:
// CHECK: add e
// CHECK-NOT: add r

int main() {
    int a = 20;
    int b = 22;
    return a + b;
}
