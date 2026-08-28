// `~x` needs no instruction of its own: flipping every bit is an exclusive-or
// against a mask of ones, and one of those does the whole job.

// CHECK-LABEL: complement:
// CHECK: xor e
// CHECK-NOT: xor

int complement(int a) {
    return ~a;
}

int main() {
    return complement(-43);
}
