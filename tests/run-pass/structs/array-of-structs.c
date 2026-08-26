// An array whose elements are structs. The element size is not one the machine
// can scale an index by, so the index is multiplied into a byte offset.
//@ exit-code: 119

struct Item {
    int id;
    char tag;
};

int main() {
    struct Item items[4];
    int i;
    for (i = 0; i < 4; i = i + 1) {
        items[i].id = i * 10;
        items[i].tag = 'a' + i;
    }

    int sum = 0;
    for (i = 0; i < 4; i = i + 1) {
        sum = sum + items[i].id + (items[i].tag - 'a');
    }

    int *p = &items[2].id;
    *p = *p + 1;

    struct Item *e = &items[3];
    e->id = e->id + 2;

    return sum + items[2].id + items[3].id;
}
