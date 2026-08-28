// Every value here is still needed after the shift, so the register the count
// has to travel in is holding one of them. Whatever was in it must survive.
//@ exit-code: 251

int mix(int a, int b, int c, int d, int e, int f) {
    int shifted = a << (b & 3);
    return shifted + a + b + c + d + e + f;
}

int main() {
    return mix(10, 3, 40, 50, 60, 8);
}
