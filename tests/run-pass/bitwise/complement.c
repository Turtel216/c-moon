// `~x` flips every bit, which for a signed value is -(x + 1).
//@ exit-code: 41

int main() {
    int a = 5;
    int flipped = ~a;

    if (flipped != -6) {
        return 1;
    }
    return ~(-42);
}
