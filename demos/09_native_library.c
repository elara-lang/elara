/* Native library fixture. bin/elara dlopens this and calls elara_open_lib. */

#include "elara.h"

elara_Status elara_open_lib(elara_State *S, elara_Value *out_exports) {
    elara_Value table;
    elara_Status st;

    if (out_exports == NULL) {
        return ELARA_ERR_RUNTIME;
    }
    table = elara_new_table(S);
    if (elara_is_nil(table)) {
        return ELARA_ERR_OOM;
    }
    st = elara_table_set(S, table, "answer", elara_val_i48(42));
    if (st != ELARA_OK) {
        return st;
    }
    *out_exports = table;
    return ELARA_OK;
}
