// Elements of a `long int` array are eight bytes apart, so a value needing
// more than 32 bits survives a round trip through one.
//@ exit-code: 7

int main() {
    long int values[3];
    int i = 0;
    while (i < 3) {
        values[i] = 3000000000;
        i = i + 1;
    }
    long int total = values[0] + values[1] + values[2];
    return total / 1285714285;
}
