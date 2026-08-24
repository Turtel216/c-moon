// The unsigned types cross a call boundary like any other, narrowing on the
// way in and widening on the way out.
//@ exit-code: 44

unsigned char low_byte(unsigned int value) {
    return value;
}

unsigned int widen(unsigned char byte) {
    return byte;
}

int main() {
    unsigned char byte = low_byte(4294967084);
    if (widen(byte) != 44) {
        return 1;
    }
    return byte;
}
