// Widening an `unsigned char` fills with zeroes, which is `movzx`; the
// `movsx` a plain `char` needs would make 200 into -56.

// CHECK-LABEL: widen:
// CHECK: movzx
// CHECK-NOT: movsx

unsigned int widen(unsigned char byte) {
    return byte;
}

int main() {
    return widen(200) - 100;
}
