// Both operands are known, so SCCP settles the answer and prunes the branch
// each short-circuit needed -- leaving neither a test nor the blocks the
// unevaluated operand would have lived in.
//@ compile-flags: --opt

// CHECK-LABEL: main:
// CHECK-NOT: test
// CHECK-NOT: jne
// CHECK: mov rax, 1

int main() {
    int a = 1;
    int b = 0;

    return (a && !b) || b;
}
