// With `a` known to be 2 the condition is decided at compile time, so the
// branch is eliminated along with the dead arm.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: cmp
// CHECK-NOT: je
// CHECK: mov rax, 4

int main() {
    int a = 2;

    if (a == 2) {
        a = a + 1;
    } else {
        a = 0;
    }

    return a + 1;
}
