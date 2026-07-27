// An odd number of stack arguments needs eight bytes of alignment padding,
// which must sit below them so argument seven stays at [rbp + 16].
//@ exit-code: 28

int weighted(int a, int b, int c, int d, int e, int f, int g) {
    return a * 1 + b * 2 + c * 3 + d * 4 + e * 5 + f * 6 + g * 7;
}

int main() {
    return weighted(1, 1, 1, 1, 1, 1, 1);
}
