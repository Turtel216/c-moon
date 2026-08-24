// Widening an `unsigned char` fills the bits above it with zeroes, where
// widening a plain `char` copies its top bit.
//@ exit-code: 200

int main() {
    unsigned char unsigned_byte = 200;
    char signed_byte = 200;

    int widened = unsigned_byte;
    if (signed_byte > 0) {
        return 1;
    }
    if (signed_byte != 0 - 56) {
        return 2;
    }
    return widened;
}
