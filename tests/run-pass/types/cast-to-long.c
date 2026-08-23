// A cast widens before the multiplication rather than after it, so the product
// is computed at 64 bits and does not overflow the way `int` arithmetic would.
//@ exit-code: 4

int main() {
    int side = 2000000;
    long int area = (long int) side * side;
    return area / 1000000000000;
}
