export type ReceiptReference={key:number;plan:string;revision:string};
type Lookup=Omit<ReceiptReference,'key'>;
const database='omloom-receipt-recovery',storeName='pending';

function transaction<T>(mode:IDBTransactionMode,work:(store:IDBObjectStore,done:(value:T)=>void,fail:(error:unknown)=>void)=>void):Promise<T>{
  return new Promise((resolve,reject)=>{
    let blocked=false;
    const request=indexedDB.open(database,1);
    request.onupgradeneeded=()=>request.result.createObjectStore(storeName,{autoIncrement:true});
    request.onerror=()=>reject(request.error);
    request.onblocked=()=>{blocked=true;reject(Error('Close other receipt editors before opening recovery storage.'));};
    request.onsuccess=()=>{
      const db=request.result;
      if(blocked){db.close();return;}
      let tx:IDBTransaction;
      try{tx=db.transaction(storeName,mode,mode==='readwrite'?{durability:'strict'}:undefined);}
      catch(e){db.close();reject(e);return;}
      let result:T,failure:unknown;
      tx.oncomplete=()=>{db.close();resolve(result);};
      tx.onabort=()=>{db.close();reject(failure===undefined?tx.error:failure);};
      try{work(tx.objectStore(storeName),value=>{result=value;},error=>{failure=error;tx.abort();});}
      catch(e){failure=e;tx.abort();}
    };
  });
}

function lookup(value:unknown):Lookup{
  if(typeof value!=='object'||value===null||!('plan'in value)||!('revision'in value)||typeof value.plan!=='string'||typeof value.revision!=='string'||!/^([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/.test(value.plan)||!/^\d{1,20}$/.test(value.revision))throw Error('The saved receipt reference is invalid.');
  return {plan:value.plan,revision:value.revision};
}

export function readReceiptReference():Promise<ReceiptReference|undefined>{
  return transaction('readonly',(store,done,fail)=>{
    const request=store.openCursor();
    request.onsuccess=()=>{
      const cursor=request.result;
      if(!cursor){done(undefined);return;}
      try{
        if(typeof cursor.key!=='number')throw Error('The saved receipt key is invalid.');
        done({key:cursor.key,...lookup(cursor.value)});
      }catch(e){fail(e);}
    };
  });
}

export function claimReceiptReference(value:Lookup):Promise<ReceiptReference|undefined>{
  // IndexedDB 2.0 section 2.7: overlapping readwrite transactions execute serially.
  // Keep one unresolved reference; the generated key fences delayed cross-tab cleanup.
  return transaction('readwrite',(store,done)=>{
    const request=store.openKeyCursor();
    request.onsuccess=()=>{
      if(request.result){done(undefined);return;}
      const added=store.add(value);
      added.onsuccess=()=>done({key:added.result as number,...value});
    };
  });
}

export function clearReceiptReference(value:ReceiptReference):Promise<void>{
  return transaction('readwrite',(store,done)=>{store.delete(value.key);done(undefined);});
}
