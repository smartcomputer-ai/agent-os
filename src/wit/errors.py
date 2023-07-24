
class InvalidCoreException(Exception):
    pass

class InvalidWitException(Exception):
    pass

class InvalidMessageException(Exception):
    pass

class InvalidUpdateException(Exception):
    pass

class QueryError(Exception):
    query_not_found:bool = False
    pass