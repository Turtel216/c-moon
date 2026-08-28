// A left shift moves bits towards the top and fills with zeroes.
//@ exit-code: 40

int main() {
    int a = 5;
    int b = 3;
    return a << b;
}
