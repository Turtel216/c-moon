// Mixes an address-taken parameter, which is pinned to a stack slot, with
// register- and stack-passed arguments.
//@ exit-code: 29

int bump(int *p, int b, int c, int d, int e, int f, int g, int h) {
    *p = *p + 1;
    return *p + b + c + d + e + f + g + h;
}

int main() {
    int v = 100;
    return bump(&v, 1, 2, 3, 4, 5, 6, 7) - 100;
}
