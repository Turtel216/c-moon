// Bitwise operands that are calls, so the values meet across a call boundary
// rather than sitting in registers throughout.
//@ exit-code: 42

int low() {
    return 40;
}

int high() {
    return 2;
}

int main() {
    return (low() | high()) & 63;
}
