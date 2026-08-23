// A `long int` carried around a loop stays 64 bits wide across the phi nodes
// the loop header needs.
//@ exit-code: 6

long int accumulate(long int seed, int times) {
    long int total = seed;
    int i = 0;
    while (i < times) {
        total = total + seed;
        i = i + 1;
    }
    return total;
}

int main() {
    long int total = accumulate(1000000000, 5);
    return total / 1000000000;
}
