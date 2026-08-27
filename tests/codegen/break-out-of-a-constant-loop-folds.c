// A loop whose only way out is a `break` reached on its first iteration is
// not a loop at run time: with the condition folded away, nothing is tested
// and nothing branches back.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: test
// CHECK-NOT: while
// CHECK: mov rax, 7

int main() {
    int n = 7;

    while (1) {
        break;
    }

    return n;
}
