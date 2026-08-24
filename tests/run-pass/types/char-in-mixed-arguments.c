// A `char` keeps its place among arguments of other types, both in the
// registers the ABI hands out first and in the stack slots after them.
//@ exit-code: 45

int mix(char a, int b, char c, long int d, char e, int f, char g, long int h, char i) {
    return a + b + c + d + e + f + g + h + i;
}

int main() {
    return mix(1, 2, 3, 4, 5, 6, 7, 8, 9);
}
