// A `break` ends its block, so the statements written after it are reachable
// by nothing and are deleted with the block they landed in -- no instruction
// carrying the returned constant survives, even unoptimised.

// CHECK-LABEL: main:
// CHECK-NOT: 12345
// CHECK: mov rax, 7

int main() {
    while (1) {
        break;
        return 12345;
    }

    return 7;
}
