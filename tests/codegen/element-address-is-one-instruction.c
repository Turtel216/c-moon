// The address of an element is the memory operand an access to it would have
// used, so it costs a single `lea` and no index arithmetic of its own.
//@ compile-flags: --opt

// CHECK-LABEL: element:
// CHECK-NOT: imul
// CHECK: lea

int element(int i) {
    long int values[4];
    values[i] = 7;
    long int *slot = &values[i];
    return *slot;
}

int main() {
    return element(3);
}
