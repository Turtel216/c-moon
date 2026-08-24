// Unsigned arithmetic wraps around instead of going negative: there is no
// sign bit to set, so one less than zero is the largest value there is.
//@ exit-code: 3

int main() {
    unsigned int u = 0;
    u = u - 1;

    unsigned char b = 0;
    b = b - 1;

    return (u == 4294967295) + (b == 255) * 2;
}
